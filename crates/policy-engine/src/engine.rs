//! Motor principal del policy-engine.
//!
//! `PolicyEngine` orquesta la ejecución de todos los checks de un `PolicyBundle`.
//! Recibe el bundle del servidor (vía `grpc-client`), ejecuta cada `PolicyCheck`
//! con su executor correspondiente y devuelve los resultados como tipos proto
//! listos para encolar en `local-db` y enviar al servidor.

use std::collections::HashMap;
use std::sync::Arc;

use proto::{CheckResult, PolicyBundle};

use crate::executor::CheckExecutor;
use crate::executors::register_all_executors;
use crate::result::EngineCheckResult;

/// Motor de ejecución de checks de política.
///
/// Es stateless y barato de clonar: el `HashMap` de executors se comparte
/// mediante `Arc`. Se inicializa una sola vez al arrancar el agente.
#[derive(Clone)]
pub struct PolicyEngine {
    executors: HashMap<&'static str, Arc<dyn CheckExecutor>>,
}

impl PolicyEngine {
    /// Crea un nuevo `PolicyEngine` con todos los executors registrados.
    pub fn new() -> Self {
        Self {
            executors: register_all_executors(),
        }
    }

    /// Ejecuta todos los checks de todas las políticas del bundle.
    ///
    /// Los checks se ejecutan secuencialmente para no saturar los recursos del
    /// sistema. En un agente real con docenas de checks, el tiempo total es
    /// despreciable comparado con el intervalo de poll.
    ///
    /// Nunca falla: los checks que producen errores internos se convierten en
    /// resultados `passed = false` con el detalle del error, para que el servidor
    /// siempre reciba un resultado por cada check del bundle.
    pub async fn run_all(&self, bundle: &PolicyBundle) -> Vec<CheckResult> {
        let mut results = Vec::new();

        for policy in &bundle.policies {
            for element in &policy.elements {
                for check in &element.checks {
                    let result = self.run_check(&check.id, &check.check_type, &check.check_params_json).await;
                    results.push(result);
                }
            }
        }

        tracing::debug!(
            total_checks = results.len(),
            passed = results.iter().filter(|r| r.passed).count(),
            failed = results.iter().filter(|r| !r.passed).count(),
            "ejecución del bundle completada"
        );

        results
    }

    /// Ejecuta un check individual por su tipo e ID.
    ///
    /// Convierte cualquier error interno en un `CheckResult` con `passed = false`.
    pub async fn run_check(
        &self,
        check_id: &str,
        check_type: &str,
        check_params_json: &str,
    ) -> CheckResult {
        let params = match serde_json::from_str::<serde_json::Value>(check_params_json) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    check_id,
                    check_type,
                    error = %e,
                    "params JSON invalido para check"
                );
                return error_result(check_id, format!("params JSON invalido: {}", e));
            }
        };

        let executor = match self.executors.get(check_type) {
            Some(e) => e,
            None => {
                tracing::warn!(check_id, check_type, "tipo de check no soportado");
                return error_result(
                    check_id,
                    format!("tipo de check '{}' no soportado por esta version del agente", check_type),
                );
            }
        };

        tracing::debug!(check_id, check_type, "ejecutando check");

        match executor.execute(check_id, &params).await {
            Ok(engine_result) => {
                let passed = engine_result.passed;
                let result: CheckResult = engine_result.into();

                if passed {
                    tracing::debug!(check_id, check_type, "check PASS: {}", result.detail);
                } else {
                    tracing::info!(check_id, check_type, "check FAIL: {}", result.detail);
                }

                result
            }
            Err(e) => {
                tracing::warn!(check_id, check_type, error = %e, "error ejecutando check");
                error_result(check_id, format!("error al ejecutar el check: {}", e))
            }
        }
    }

    /// Devuelve los `check_type` soportados por este engine.
    /// Útil para logging de diagnóstico al arrancar.
    pub fn supported_check_types(&self) -> Vec<&'static str> {
        let mut types: Vec<&'static str> = self.executors.keys().copied().collect();
        types.sort();
        types
    }
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Construye un CheckResult de error (passed = false) con el timestamp actual.
fn error_result(check_id: &str, detail: impl Into<String>) -> CheckResult {
    use std::time::{SystemTime, UNIX_EPOCH};
    let executed_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    CheckResult {
        check_id: check_id.to_string(),
        passed: false,
        detail: detail.into(),
        actual_value: String::new(),
        expected_value: String::new(),
        executed_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto::{Policy, PolicyBundle, PolicyCheck, PolicyElement};
    use tempfile::tempdir;

    fn make_bundle_with_check(check_type: &str, params_json: &str) -> PolicyBundle {
        PolicyBundle {
            bundle_hash: "test-hash".into(),
            policies: vec![Policy {
                id: "pol-1".into(),
                name: "Test Policy".into(),
                version: "1.0".into(),
                severity: "high".into(),
                elements: vec![PolicyElement {
                    id: "el-1".into(),
                    name: "Test Element".into(),
                    checks: vec![PolicyCheck {
                        id: "chk-1".into(),
                        name: "Test Check".into(),
                        check_type: check_type.into(),
                        check_params_json: params_json.into(),
                        regulation_section_ids: vec![],
                        remediation: None,
                    }],
                }],
            }],
        }
    }

    #[test]
    fn engine_has_all_expected_check_types() {
        let engine = PolicyEngine::new();
        let types = engine.supported_check_types();

        for expected in &[
            "file_exists", "file_absent", "dir_contains", "symlink",
            "file_line", "file_block", "ini_value",
            "pkg_installed", "pkg_absent",
            "sysctl", "service", "user_attr",
        ] {
            assert!(types.contains(expected), "falta check type: {}", expected);
        }
    }

    #[tokio::test]
    async fn run_check_unsupported_type_returns_error_result() {
        let engine = PolicyEngine::new();
        let result = engine.run_check("chk-1", "tipo_inventado", "{}").await;
        assert!(!result.passed);
        assert!(result.detail.contains("tipo_inventado"));
    }

    #[tokio::test]
    async fn run_check_invalid_params_json_returns_error() {
        let engine = PolicyEngine::new();
        let result = engine.run_check("chk-1", "file_exists", "no es json").await;
        assert!(!result.passed);
        assert!(result.detail.contains("JSON"));
    }

    #[tokio::test]
    async fn run_all_empty_bundle_returns_empty() {
        let engine = PolicyEngine::new();
        let bundle = PolicyBundle { bundle_hash: "h".into(), policies: vec![] };
        let results = engine.run_all(&bundle).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn run_all_file_absent_check() {
        let engine = PolicyEngine::new();
        let bundle = make_bundle_with_check(
            "file_absent",
            r#"{"path": "/tmp/complyx_test_engine_absent_xyz_99"}"#,
        );

        let results = engine.run_all(&bundle).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }

    #[tokio::test]
    async fn run_all_file_line_check() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("login.defs");
        tokio::fs::write(&path, "PASS_MIN_LEN    15\n").await.unwrap();

        let params = serde_json::json!({
            "path": path,
            "key": "PASS_MIN_LEN",
            "operator": ">=",
            "value": "15"
        });

        let engine = PolicyEngine::new();
        let bundle = make_bundle_with_check("file_line", &params.to_string());
        let results = engine.run_all(&bundle).await;

        assert_eq!(results.len(), 1);
        assert!(results[0].passed, "esperado pass: {}", results[0].detail);
    }
}
