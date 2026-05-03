//! Motor principal del remediation-engine.
//!
//! `RemediationEngine` orquesta el ciclo completo de una remediación:
//! 1. Abre la entrada de auditoría (`pending`) antes de actuar.
//! 2. Delega la ejecución al remediator correspondiente.
//! 3. Cierra la entrada de auditoría con el resultado final.
//!
//! El motor nunca se llama solo: el `poll_loop` de `agent-core` decide si una
//! remediación debe ejecutarse (basándose en el resultado del check y en si la
//! política autoriza la remediación automática).

use std::collections::HashMap;
use std::sync::Arc;

use local_db::LocalDb;
use proto::PolicyRemediation;

use crate::audit::AuditWriter;
use crate::executor::{RemediationError, RemediationExecutor};
use crate::remediators::register_all_remediators;

// Resultado público de ejecutar una remediación, devuelto al caller.
#[derive(Debug)]
pub struct RemediationOutcome {
    pub remediation_id: String,
    pub remediation_type: String,
    pub applied: bool,
    pub detail: String,
}

// Motor de ejecución de remediaciones.
//
// Es barato de clonar: el `HashMap` de remediators y el `AuditWriter`
// se comparten mediante `Arc`.
#[derive(Clone)]
pub struct RemediationEngine {
    remediators: HashMap<&'static str, Arc<dyn RemediationExecutor>>,
    audit: Arc<AuditWriter>,
}

impl RemediationEngine {
    // Crea un nuevo `RemediationEngine` con todos los remediators registrados.
    pub fn new(db: LocalDb) -> Self {
        Self {
            remediators: register_all_remediators(),
            audit: Arc::new(AuditWriter::new(db)),
        }
    }

    // Ejecuta la remediación asociada a un check fallido.
    //
    // # Argumentos
    //
    // * `check_id` — UUID del `PolicyCheck` cuya remediación se va a aplicar.
    // * `remediation` — `PolicyRemediation` del bundle (contiene tipo y params JSON).
    //
    // # Flujo
    //
    // 1. Verifica que el tipo de remediación está soportado.
    // 2. Abre entrada de auditoría (`pending`).
    // 3. Ejecuta el remediator.
    // 4. Cierra la entrada de auditoría con el resultado.
    // 5. Devuelve el `RemediationOutcome` al caller.
    //
    // Los errores del remediator se capturan y se devuelven como `Ok(outcome)`
    // con `applied = false` — el caller nunca recibe un `Err` por fallos de la
    // remediación en sí, solo por errores de infraestructura (auditoría, BD).
    pub async fn apply(&self, check_id: &str, remediation: &PolicyRemediation,) -> Result<RemediationOutcome, crate::EngineError> {
        let rem_type = &remediation.remediation_type;

        // 1. Verificar soporte
        let executor = match self.remediators.get(rem_type.as_str()) {
            Some(e) => Arc::clone(e),
            None => {
                tracing::warn!(
                    check_id,
                    remediation_id = %remediation.id,
                    remediation_type = %rem_type,
                    "tipo de remediación no soportado"
                );
                return Ok(RemediationOutcome {
                    remediation_id: remediation.id.clone(),
                    remediation_type: rem_type.clone(),
                    applied: false,
                    detail: format!(
                        "tipo '{}' no soportado por esta versión del agente",
                        rem_type
                    ),
                });
            }
        };

        // 2. Parsear params JSON
        let params: serde_json::Value =
            serde_json::from_str(&remediation.remediation_params_json).map_err(|e| {
                crate::EngineError::InvalidParams {
                    remediation_type: rem_type.clone(),
                    reason: e.to_string(),
                }
            })?;

        // 3. Abrir auditoría
        let handle = self
            .audit
            .begin(check_id, &remediation.id, rem_type, &remediation.remediation_params_json,)
            .await
            .map_err(crate::EngineError::Audit)?;

        tracing::info!(
            check_id,
            remediation_id = %remediation.id,
            remediation_type = %rem_type,
            audit_id = %handle.audit_id(),
            "aplicando remediación"
        );

        // 4. Ejecutar
        let exec_result = executor.execute(&remediation.id, &params).await;

        // 5. Traducir a outcome y cerrar auditoría
        let outcome = match &exec_result {
            Ok(r) => RemediationOutcome {
                remediation_id: remediation.id.clone(),
                remediation_type: rem_type.clone(),
                applied: r.applied,
                detail: r.detail.clone(),
            },
            Err(e) => {
                tracing::error!(
                    check_id,
                    remediation_id = %remediation.id,
                    error = %e,
                    "remediación fallida"
                );
                RemediationOutcome {
                    remediation_id: remediation.id.clone(),
                    remediation_type: rem_type.clone(),
                    applied: false,
                    detail: e.to_string(),
                }
            }
        };

        // Convertir para el log de auditoría
        let audit_result: Result<crate::executor::RemediationResult, String> = match exec_result {
            Ok(r) => Ok(r),
            Err(e) => Err(e.to_string()),
        };
        handle.commit(&audit_result).await;

        Ok(outcome)
    }

    // Devuelve los tipos de remediación soportados. Para logging de diagnóstico.
    pub fn supported_types(&self) -> Vec<&'static str> {
        let mut types: Vec<&'static str> = self.remediators.keys().copied().collect();
        types.sort();
        types
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use local_db::LocalDb;
    use proto::PolicyRemediation;
    use tempfile::tempdir;

    async fn make_engine() -> RemediationEngine {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        // Leak the tempdir so it stays alive for the test
        std::mem::forget(dir);
        let db = LocalDb::connect(db_path).await.unwrap();
        RemediationEngine::new(db)
    }

    #[tokio::test]
    async fn apply_unsupported_type_returns_outcome_not_applied() {
        let engine = make_engine().await;

        let rem = PolicyRemediation {
            id: "rem-1".into(),
            remediation_type: "tipo_inventado".into(),
            remediation_params_json: "{}".into(),
        };

        let outcome = engine.apply("chk-1", &rem).await.unwrap();
        assert!(!outcome.applied);
        assert!(outcome.detail.contains("no soportado"));
    }

    #[tokio::test]
    async fn apply_invalid_params_json_returns_engine_error() {
        let engine = make_engine().await;

        let rem = PolicyRemediation {
            id: "rem-1".into(),
            remediation_type: "file_line_set".into(),
            remediation_params_json: "no es json".into(),
        };

        let result = engine.apply("chk-1", &rem).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn engine_has_all_expected_types() {
        let engine = make_engine().await;
        let types = engine.supported_types();

        for expected in &["file_line_set", "file_block_set", "pkg_install",
                          "pkg_remove", "sysctl_set", "service_set"] {
            assert!(types.contains(expected), "falta tipo: {}", expected);
        }
    }

    #[tokio::test]
    async fn apply_file_line_set_on_real_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("login.defs");
        tokio::fs::write(&path, "PASS_MIN_LEN 8\n").await.unwrap();

        let engine = make_engine().await;

        let params = serde_json::json!({
            "path": path,
            "key": "PASS_MIN_LEN",
            "value": "15",
            "backup": false
        });

        let rem = PolicyRemediation {
            id: "rem-1".into(),
            remediation_type: "file_line_set".into(),
            remediation_params_json: params.to_string(),
        };

        let outcome = engine.apply("chk-1", &rem).await.unwrap();
        assert!(outcome.applied, "esperado applied: {}", outcome.detail);

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("PASS_MIN_LEN 15"));
    }
}
