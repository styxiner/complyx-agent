//! Construccion de la configuracion mTLS para el cliente gRPC
//!
//! Lee los 3 ficheros del directorio de certificados del agente:
//! * `agent.crt`: Certificado del agente firmado por CA interna
//! * `agent.key`: Clave privada del agente
//! * `ca.crt`: Certificado raiz de la CA interna del servidr
//!
//! El certificado de cliente prueba la identidad del agente al servidor. El certificado raiz
//! permite al agente verificar que está hablando con le serivor legítimo de Complyx

use std::path::Path;

use rustls::{pki_types::{CertificateDer, PrivateKeyDer}, ClientConfig, RootCertStore};
use rustls_pemfile::{certs, private_key};
use tonic::transport::{Certificate, ClientTlsConfig, Identity};

// Errores especificos de la configuracion TLS
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("No se encontro ningun certificado en {path}")]
    NoCertificate {path: String},

    #[error("No se encontró ninguna clave privada en {path}")]
    NoPrivateKey {path: String},

    #[error("Error al leer {path}: {source}")]
    ReadError {
        path: String,

        #[source]
        source: std::io::Error,
    },

    #[error("Certitificado PEM inválido en {path}: {reason}")]
    InvalidPem {path: String, reason: String},
}

// Rutas a los 3 ficheros que componen la entidad mTLS del agente
pub struct CertPaths {
    pub agent_cert: std::path::PathBuf,
    pub agent_key: std::path::PathBuf,
    pub ca_cert: std::path::PathBuf,
}

impl CertPaths {
    // Construye las rutas a partir del directorio de certificados configurado. 
    // Espera encontrar los ficheros agent.crt, agent.key y ca.crt dentro de cert_dir
    pub fn from_dir(cert_dir: impl AsRef<Path>) -> Self {
        let dir = cert_dir.as_ref();

        Self {
            agent_cert: dir.join("agent.crt"),
            agent_key: dir.join("agent.key"),
            ca_cert: dir.join("ca.crt"),
        }
    }

    // Devuelve true si los 3 ficheros existen.
    // Se usa en el arranque para detectar si el agente ya está registrado.
    pub fn all_exist(&self) -> bool {
        self.agent_cert.exists() && 
        self.agent_key.exists() && 
        self.ca_cert.exists()
    }
}

// Carga los ficheros de certificados y construye el `ClientTlsConfig` de Tonic listo para pasarle
// el `Channel` gRPC
//
// Errores:
// * Devuelve `TlsError` si alguno de los ficheros no existe, no se puede leer o contiene datos PEM
// inválidos.
pub async fn build_tls_config(paths: &CertPaths) -> Result<ClientTlsConfig, TlsError> {
    let agent_cert_pem = read_file(&paths.agent_cert).await?;
    let agent_key_pem = read_file(&paths.agent_key).await?;
    let ca_cert_pem = read_file(&paths.ca_cert).await?;

    // Verifica que los PEM son parseables antes de pasarlos a Tonic, asi el error es descriptivo en
    // vez de un panic interno de rustls
    validate_cert_pem(&agent_cert_pem, &paths.agent_cert)?;
    validate_key_pem(&agent_key_pem, &paths.agent_key)?;
    validate_cert_pem(&ca_cert_pem, &paths.ca_cert)?;

    // Identity = cert de cliente + clave privada
    let identity = Identity::from_pem(&agent_cert_pem, &agent_key_pem);

    // CA raiz para verificar certificado del servidor
    let ca_cert = Certificate::from_pem(&ca_cert_pem);

    let tls_config = ClientTlsConfig::new().identity(identity).ca_certificate(ca_cert);

    tracing::debug!(
        agent_cert = %paths.agent_cert.display(),
        ca_cert = %paths.ca_cert.display(),
        "Configuracion mTLS construida correctamente"
        );

    Ok(tls_config)
}

// helpers para formar la configuracion mTLS

async fn read_file(path: &std::path::Path) -> Result<Vec<u8>, TlsError> {
    tokio::fs::read(path).await.map_err(|e| TlsError::ReadError {
        path: path.display().to_string(),
        source: e,
    })
}

fn validate_cert_pem(pem: &[u8], path: &std::path::Path) -> Result<(), TlsError> {
    let mut cursor = std::io::Cursor::new(pem);
    let certs: Vec<_> = certs(&mut cursor).collect();

    if certs.is_empty() || certs.iter().all(|c| c.is_err()) {
        return Err(TlsError::NoCertificate {
            path: path.display().to_string(),
        });
    }

    if let Some(Err(e)) = certs.into_iter().find(|c| c.is_err()) {
        return Err(TlsError::InvalidPem {
            path: path.display().to_string(),
            reason: e.to_string(),
        });
    }

    Ok(())
}

fn validate_key_pem(pem: &[u8], path: &std::path::Path) -> Result<(), TlsError> {
    let mut cursor = std::io::Cursor::new(pem);
    match private_key(&mut cursor) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(TlsError::NoPrivateKey {
            path: path.display().to_string(),
        }),
        Err(e) => Err(TlsError::InvalidPem {
            path: path.display().to_string(),
            reason: e.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn cert_paths_from_dir() {
        let paths = CertPaths::from_dir("/var/lib/complyx/certs");
        assert_eq!(paths.agent_cert, PathBuf::from("/var/lib/complyx/certs/agent.crt"));
        assert_eq!(paths.agent_key, PathBuf::from("/var/lib/complyx/certs/agent.key"));
        assert_eq!(paths.ca_cert, PathBuf::from("/var/lib/complyx/certs/ca.crt"));
    }

    #[test]
    fn all_exist_returns_false_when_missing() {
        let paths = CertPaths::from_dir("/tmp/this_dir_does_not_exist_complyx_test");
        assert!(!paths.all_exist());
    }
}
