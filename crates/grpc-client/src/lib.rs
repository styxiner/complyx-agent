//! Cliente gRPC del agente complyx:
//! Registro inicial, polling de politicas y envio de resultados.

mod client;
mod mtls;
mod retry;
pub mod enroll;

pub use client::{GrpcClient, GrpcClientConfig};
pub use retry::RetryPolicy;

// Re exportar los tipos proto que el agente necesita conocer para no obligar a los crates
// consumidores a importar `proto` directamente

pub use proto::complyx::{CheckResult, Policy, PolicyBundle, PollResponse, SubmitResultsResponse};

// Errores que puede producir el cliente gRPC
#[derive(Debug, thiserror::Error)]
pub enum GrpcError {
    // El servidor devuelve un error gRPC con codigo y mensaje
    #[error("Error gRPC ({code}): {message}")]
    Status {code: String, message: String},

    // No se puede conectar al servidor despues de todos los reintentos
    #[error("no se ha podido conectar al servidor tras {attempts} intentos: {source}")]
    ConnectionFailed {
        attempts: u32,

        #[source]
        source: tonic::transport::Error
    },

    // Error al cargar la configuracion TLS
    #[error("Error de configuracion TLS: {0}")]
    Tls(#[from] mtls::TlsError),

    // Error al leer ficheros del sistema de ficheros (certificados o claves).
    #[error("error de E/S: {0}")]
    Tls(#[from] std::io::Error),
}

impl From<tonic::Status> for GrpcError {
    fn from(s: tonic::Status) -> Self {
        GrpcError::Status {
            code: format!("{:?}", s.code()),
            message: s.message().to_string(),
        }
    }
}
