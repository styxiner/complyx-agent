//! Trait base para todos los remediators y tipo `RemediationResult`.

use async_trait::async_trait;

// Resultado de aplicar una remediación.
#[derive(Debug, Clone)]
pub struct RemediationResult {
    pub applied: bool,

    pub detail: String,
}

impl RemediationResult {
    pub fn applied(detail: impl Into<String>) -> Self {
        Self { 
            applied: true, 
            detail: detail.into() 
        }
    }

    pub fn skipped(reason: impl Into<String>) -> Self {
        Self { 
            applied: false, 
            detail: reason.into() 
        }
    }
}

// Error que puede producir un remediator al intentar aplicar el cambio.
#[derive(Debug, thiserror::Error)]
pub enum RemediationError {
    #[error("parámetros inválidos para remediación '{remediation_type}': {reason}")]
    InvalidParams { remediation_type: String, reason: String },

    #[error("permisos insuficientes para modificar '{path}'")]
    PermissionDenied { path: String },

    #[error("error de I/O en '{path}': {reason}")]
    IoError { path: String, reason: String },

    #[error("tipo de remediación '{0}' no soportado por esta versión del agente")]
    UnsupportedType(String),

    #[error("error interno en remediación '{remediation_type}': {reason}")]
    Internal { remediation_type: String, reason: String },
}

impl RemediationError {
    pub fn invalid_params(remediation_type: &str, err: serde_json::Error) -> Self {
        Self::InvalidParams {
            remediation_type: remediation_type.to_string(),
            reason: err.to_string(),
        }
    }

    pub fn io(path: impl Into<String>, err: std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::PermissionDenied {
            return Self::PermissionDenied { path: path.into() };
        }
        Self::IoError {
            path: path.into(),
            reason: err.to_string(),
        }
    }
}

// Interfaz que debe implementar cada remediator.
//
// Los remediators son stateless: no guardan estado entre ejecuciones.
// Toda la información necesaria viene en `params`.
//
// Un remediator debe asegurar lo siguiente:
//
// - Nunca debe hacer panic. Los errores se devuelven como `Err(RemediationError)`.
// - No debe hacer I/O de red.
// - Antes de modificar un fichero, siempre debe hacer backup.
// - Debe verificar que tiene los permisos necesarios antes de actuar.
// - Debe ser idempotente: aplicar la misma remediación dos veces debe tener el
//   mismo resultado que aplicarla una vez.
#[async_trait]
pub trait RemediationExecutor: Send + Sync {
    // Identificador del tipo de remediación. Debe coincidir con `remediation_type`
    // en el `PolicyRemediation` del proto.
    fn remediation_type(&self) -> &'static str;

    // Aplica la remediación con los parámetros dados.
    async fn execute(&self, remediation_id: &str, params: &serde_json::Value,) -> Result<RemediationResult, RemediationError>;
}
