//! # remediation-engine
//!
//! Librería que aplica correcciones al sistema local cuando un check de política falla.
//!
//! ## Relación con policy-engine
//! 
//! El `policy-engine` detecta incumplimientos (actúa de forma de solo lectura sin modificar el
//! sistema). El `remediation-engine` los corrige (escribiendo en ficheros, instalando paquetes..).
//! El `agent-core` decide si llamar al `remediation-engine` basándose en el resultado del check y
//! en si la politica autoriza la remediacion automatica.
//!
//! Tuene que cumplir lo siguiente:
//!
//! * Registro de auditoria: Toda remediacion se registra en `local-db` antes y despues de ejecutarse. Si
//! el agente muere a mitad, queda una entrada `pending`.
//! * Backup: Los ejecutores `remediators` qie ,pdofocam ficheros hacen copia de seguridad
//! automatica con extension `.back.complyx` antes de escribir.
//! * Idempotencia: Aplicar la misma remediacion dos veces produce el mismo resultado que aplicarla
//! una vez.
//! * Sin shell arbitrario: Los `remediators` invocan binarios concretos con argumentos fijos.
//! Nunca ejecutan strings del servidor como comandos (para prevenir RCEs, lo cual es uno de los
//! puntos principales del proyecto xd)
//!
//! ## Uso
//! Lo pongo en ignore, que no tengo tiempo de poner un ejemplo completo.
//! ```ignore
//! use remediation_engine::RemediationEngine;
//! use local_db::LocalDb;
//! use proto::PolicyRemediation;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let db = LocalDb::connect("/var/lib/complyx/agent.db").await?;
//!     let engine = RemediationEngine::new(db);
//!
//!     let remediation = PolicyRemediation {
//!         id: "rem-001".into(),
//!         remediation_type: "file_line_set".into(),
//!         remediation_params_json: r#"{
//!             "path": "/etc/login.defs",
//!             "key": "PASS_MIN_LEN",
//!             "value": "15"
//!         }"#.into(),
//!     };
//!
//!     let outcome = engine.apply("chk-001", &remediation).await?;
//!     println!("applied: {}, detail: {}", outcome.applied, outcome.detail);
//!
//!     Ok(())
//! }
//! ```

mod audit;
mod engine;
mod executor;
mod remediators;

pub use engine::{RemediationEngine, RemediationOutcome};
pub use executor::{RemediationError, RemediationExecutor, RemediationResult};

// Error de infraestructura del engine (distinto de los errores de los remediators).
//
// Los errores de los remediators individuales se capturan y se convierten en
// `RemediationOutcome { applied: false }`. Solo los errores que impiden operar
// (auditoría rota, params JSON inválido) se propagan como `EngineError`.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("params JSON inválidos para '{remediation_type}': {reason}")]
    InvalidParams { remediation_type: String, reason: String }, // El JSON de params del `PolicyRemediation` no se pudo deserializar.

    #[error("error de auditoría: {0}")]
    Audit(#[from] local_db::LocalDbError),  // Error al escribir en el log de auditoría de `local-db`.
}
