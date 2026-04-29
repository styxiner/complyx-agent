//! Log de auditoria de las remediaciones
//!
//! El `remediation-engine` escribe en esta tabla antes y despues de aplicar cada remediacion. El
//! log es solo append: nunca se modifican ni eliminan filas, lo que lo hace apto para auditoria
//! forense.
//!
//! Flujo de escritura:
//! 1. Antes de aplicar: `start_remediation()` -> fila con status `pending`. Si el proceso muere
//!    aqui, la proxima ejecucion vera la fila `pending` y sabra que algo quedo a medias.
//!
//! 2. Tras aplicar con exito `finish_remediation(Applied)`
//! 3. Tras fallo: `finish_remediation(Failed {reason}`
//! 4. Si se decide no aplicar: `finish_remediation(Skipped {reason})`

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::LocalDbError;

// Estado final de una remediacion
pub enum RemediationOutcome {
    Applied {detail: String},
    Failed {reason: String},
    Skipped {reason: String},
}

impl RemediationOutcome {
    fn status(&self) -> &'static str {
        match self {
            Self::Applied { .. } => "applied",
            Self::Failed { .. } => "failed",
            Self::Skipped { .. } => "skipped",
        }
    }

    fn details(&self) -> &str {
        match self {
            Self::Applied { detail } => detail,
            Self::Failed { reason } => reason,
            Self::Skipped { reason } => reason,
        }
    }
}

// Abre una entrada de auditoria con estado `pending` antes de ejecutar la remediacion
//
// Devuelve el UUID de la entrada, que se pasa a `finish_remediation` cuando la remediacion
// termina.
//
// Argumentos
//
// * `check_id`: UUID que origina la remediacion
// * `remediation_id`: UUID de la `PolicyRemediation` del bundle
// * `remediation_type`: tipo de remediacion. Ej: `*file_line_set*`
// * `params_json`: parametros usados, para reproducibilidad forense

pub async fn start_remediation(pool: &SqlitePool, check_id: &str, remediation_id: &str, remediation_type: &str, params_json: &str) -> Result<String, LocalDbError> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    sqlx::query!(
        r#"
        INSERT INTO remediation_audit (id, check_id, remediation_id, remediation_type, params_json, status, started_at)
        VALUES (?, ?, ?, ?, ?, 'pending', ?)
        "#,
        id,
        check_id,
        remediation_id,
        remediation_type,
        params_json,
        now,
        )
        .execute(pool)
        .await
        .map_err(LocalDbError::Database);

    tracing::debug!(
        audit_id = %id,
        check_id,
        remediation_type,
        "entrada de auditoria de remediacion creada"
        );

    Ok(id)
}

// Cierra la entrada de auditoria con el resultado final de la remediacion
//
// Argumentos
// * `audit_id`: UUID devuelto por `start_remediation`
// * `outcome`: resultado de la remediacion

pub async fn finish_remediation(pool: &SqlitePool, audit_id: &str, outcome: RemediationOutcome) -> Result<(), LocalDbError> {
    let status = outcome.status();
    let detail = outcome.details();
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    sqlx::query!(
        r#"
        UPDATE remediation_audit
        SET status = ?, result_detail = ?, finished_at = ?
        WHERE id = ?
        "#,
        status,
        detail,
        now,
        audit_id
        )
        .execute(pool)
        .await
        .map_err(LocalDbError::Database)?;

    match &outcome {
        RemediationOutcome::Applied { .. } => {
            tracing::info!(audit_id, "remediacion aplicada con exito");
        }

        RemediationOutcome::Failed { .. } => {
            tracing::error!(audit_id, reason, "remediacion fallida");
        }

        RemediationOutcome::Skipped { .. } => {
            tracing::warn!(audit_id, reason, "remediacion omitida");
        }
    }

    Ok(())
}

// Representa una remediacion que quedo pendiente (proceso interrumpido)
pub struct PendingAudit {
    pub id: String,
    pub check_id: String,
    pub remediation_id: String,
    pub remediation_type: String,
    pub params_json: String,
    pub started_at: String
}

// Devuelve todas las entradas de auditoria con estado pendiente.
//
// Se usa en el arranque del agente para detectar las remediaciones que quedaron incompletas (el
// proceso murio entre `start_remediation()` y `finish_remediation()`).
pub async fn get_pending_audits(pool: &SqlitePool) -> Result<Vec<PendingAudit>, LocalDbError> {
    let rows = sqlx::query_as!(
        PendingAudit,
        r#"
        SELECT id, check_id, remediation_id, remediation_type, params_json, started_at
        FROM remediation_audit
        WHERE status = 'pending'
        ORDER BY started_at ASC
        "#
        )
        .fetch_all(pool)
        .await
        .map_err(LocalDbError::Database)?;

    if !rows.is_empty() {
        tracing::warn!(
            count = rows.len(),
            "remediaciones incompletas detectadas al arrancar"
            );
    }

    Ok(rows)
}








#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connect_test;
 
    #[tokio::test]
    async fn start_and_finish_applied() {
        let pool = connect_test().await;
 
        let audit_id = start_remediation(
            &pool,
            "chk-001",
            "rem-001",
            "file_line_set",
            r#"{"path":"/etc/login.defs","key":"PASS_MIN_LEN","value":"15"}"#,
        )
        .await
        .unwrap();
 
        finish_remediation(
            &pool,
            &audit_id,
            RemediationOutcome::Applied {
                detail: "PASS_MIN_LEN cambiado de 8 a 15".into(),
            },
        )
        .await
        .unwrap();
 
        let row = sqlx::query!(
            "SELECT status, result_detail, finished_at FROM remediation_audit WHERE id = ?",
            audit_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
 
        assert_eq!(row.status, "applied");
        assert!(row.result_detail.unwrap().contains("15"));
        assert!(row.finished_at.is_some());
    }
 
    #[tokio::test]
    async fn get_pending_audits_detects_incomplete() {
        let pool = connect_test().await;
 
        // Simular crash: start sin finish
        start_remediation(&pool, "chk-002", "rem-002", "sysctl_set", "{}")
            .await
            .unwrap();
 
        let pending = get_pending_audits(&pool).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].check_id, "chk-002");
    }
 
    #[tokio::test]
    async fn finished_remediation_not_in_pending() {
        let pool = connect_test().await;
 
        let id = start_remediation(&pool, "chk-003", "rem-003", "pkg_install", "{}")
            .await
            .unwrap();
 
        finish_remediation(
            &pool,
            &id,
            RemediationOutcome::Skipped {
                reason: "sin permisos de root".into(),
            },
        )
        .await
        .unwrap();
 
        let pending = get_pending_audits(&pool).await.unwrap();
        assert!(pending.is_empty());
    }
}
