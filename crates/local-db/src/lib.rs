//! Base de datos SQLite local del agente
//!
//! Responsabilidades principales:
//!
//! * Cache de politicas: Almacena el ultimo `PolicyBundle` recibido del servidor para que el
//! agente pueda seguir ejecutando checks cuando el servidor no esta alcanzable.
//!
//! * Cola de resultados: Los `CheckResult` producidos por el `policy-engine` se encolan aqui antes
//! de enviar al servidor. Si falla la red, los resultados persisten y se envian en el siguiente
//! ciclo de flush
//!
//! * Log de auditorioa: Registro inmutable de todas las remediaciones aplicadas por el
//! `remediation-engine`

mod db;
mod policy_cache;
mod remediation_audit;
mod result_queue;

pub use remediation_audit::{PendingAudit, RemediationOutcome};
pub use result_queue::QueueRow;

use std::path::Path;

use sqlx::SqlitePool;

use proto::{CheckResult, PolicyBundle}

// Errores unificados del crate
#[derive(Debug, thiserror::Error)]
pub enum LocalDbError {
    #[error("Error de base de datos: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Error de serializacion: {0}")]
    Serialization(String),

    #[error("error de migracion: {0}")]
    Migration(String),

    #[error("error de E/S en {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error
    }
}

