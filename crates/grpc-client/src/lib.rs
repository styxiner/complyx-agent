//! Cliente gRPC del agente complyx:
//! Registro inicial, polling de politicas y envio de resultados.
//!
//! ## como usar
//!
//! ```ignore
//! use grpc_client::{GrpcClient, GrpcClientConfig, enroll::EnrollRequest};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Primera vez: enrolamiento
//!     let enroll_req = EnrollRequest {
//!         token: "a3f8c2d1...".into(),
//!         csr_pem: "-----BEGIN CERTIFICATE REQUEST-----...".into(),
//!         hostname: "web-01.acme.com".into(),
//!         os_name: "Linux".into(),
//!         os_version: "6.8.0".into(),
//!     };
//!     let enroll_resp = grpc_client::enroll::enroll("https://server:9001", enroll_req).await?;
//!
//!     // Uso normal: poll + submit
//!     let config = GrpcClientConfig {
//!         server_url: "https://server:9000".into(),
//!         cert_dir: "/var/lib/complyx/certs".into(),
//!         agent_id: "550e8400-...".into(),
//!     };
//!     let client = GrpcClient::connect(config).await?;
//!
//!     let response = client.poll_policies("current-hash").await?;
//!     if response.policies_changed {
//!         // ejecutar checks con policy-engine...
//!         client.submit_results(vec![]).await?;
//!     }
//!
//!     Ok(())
//! }
//! ```


mod client;
pub mod mtls;
mod retry;
pub mod enroll;

pub use client::{GrpcClient, GrpcClientConfig};
pub use retry::RetryPolicy;

// Re exportar los tipos proto que el agente necesita conocer para no obligar a los crates
// consumidores a importar `proto` directamente

pub use proto::complyx::{CheckResult, Policy, PolicyBundle, PollResponse, SubmitResultsResponse};

// Errores que puede producir el cliente gRPC
#[derive(thiserror::Error, Debug)]
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

    #[error("error de transporte gRPC: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("url del servidor invalida: {0}")]
    InvalidUrl(#[from] tonic::codegen::http::uri::InvalidUri),

    // Error al cargar la configuracion TLS
    #[error("Error de configuracion TLS: {0}")]
    Tls(#[from] mtls::TlsError),

    // Error al leer ficheros del sistema de ficheros (certificados o claves).
    #[error("error de E/S: {0}")]
    Io(#[from] std::io::Error),
}

impl From<tonic::Status> for GrpcError {
    fn from(s: tonic::Status) -> Self {
        GrpcError::Status {
            code: format!("{:?}", s.code()),
            message: s.message().to_string(),
        }
    }
}
