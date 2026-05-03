//! Integración con el log de auditoría de `local-db`.
//!
//! El engine llama a `begin()` antes de ejecutar cada remediación y a `commit()`
//! después. Esto garantiza que cualquier cambio aplicado al sistema queda registrado
//! en la base de datos local incluso si el proceso muere a mitad — la próxima vez
//! que arranque el agente, `LocalDb::get_pending_audits()` detectará la remediación
//! incompleta.

use local_db::{LocalDb, LocalDbError, RemediationOutcome};

use crate::executor::RemediationResult;

// Handle de una entrada de auditoría abierta.
// Se obtiene de `AuditWriter::begin()` y se consume en `commit()`.
pub struct AuditHandle {
    audit_id: String,
    db: LocalDb,
}

// Escribe entradas de auditoría en `local-db`.
pub struct AuditWriter {
    db: LocalDb,
}

impl AuditWriter {
    pub fn new(db: LocalDb) -> Self {
        Self { db }
    }

    // Abre una entrada de auditoría con estado `pending` antes de ejecutar la remediación.
    //
    // Si esta función devuelve error, el engine NO debe ejecutar la remediación:
    // no tiene sentido aplicar cambios al sistema si no podemos registrarlos.
    pub async fn begin(&self, check_id: &str, remediation_id: &str, remediation_type: &str, params_json: &str,) -> Result<AuditHandle, LocalDbError> {
        let audit_id = self
            .db
            .start_remediation(check_id, remediation_id, remediation_type, params_json)
            .await?;

        Ok(AuditHandle {
            audit_id,
            db: self.db.clone(),
        })
    }
}

impl AuditHandle {
    // Cierra la entrada de auditoría con el resultado de la remediación.
    pub async fn commit(self, result: &Result<RemediationResult, String>) {
        let outcome = match result {
            Ok(r) if r.applied => RemediationOutcome::Applied {
                detail: r.detail.clone(),
            },
            Ok(r) => RemediationOutcome::Skipped {
                reason: r.detail.clone(),
            },
            Err(reason) => RemediationOutcome::Failed {
                reason: reason.clone(),
            },
        };

        if let Err(e) = self.db.finish_remediation(&self.audit_id, outcome).await {
            // No podemos hacer mucho si falla el log de auditoría.
            // Lo registramos en el log del proceso pero no propagamos el error.
            tracing::error!(
                audit_id = %self.audit_id,
                error = %e,
                "no se pudo cerrar la entrada de auditoría"
            );
        }
    }

    pub fn audit_id(&self) -> &str {
        &self.audit_id
    }
}
