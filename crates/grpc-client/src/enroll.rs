//! Flujo de registro del agente
//!
//! El registro ocurre una única vez: La primera vez que el agente arranca
//! Sin certificado, el proceso es:
//! 1. El agente genera un par de claves ed25519 con `rcgen`
//! 2. Construye una solicitud de firma de certificado (CSR) con su hostname como CN
//! 3. Llama al endpoint `ComplyxEnroll.Enroll` del servidor, siendo este un puerto separado TLS
//!    unidireccional
//! 4. El servidor valida el token, firma el CSR con su CA interna y devuelve el certificado
//! 5. El agente guarda la `agent.key`, `agent.crt` y `ca.crt`.
//!
//! A partir de ese momento, el agente usa su certificado para autenticarse con mTLS en cada llamada
//! del servicio principal


use std::path::Path;

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType, PKCS_ED25519};
use tonic::transport::{Channel, ClientTlsConfig};

//use crate::GrpcError;
use proto::complyx::complyx_enroll_client::ComplyxEnrollClient;
use proto::complyx::EnrollRequest as ProtoEnrollRequest;

// Datos para iniciar el registro
#[derive(Debug, Clone)]
pub struct EnrollRequest {
    pub token: String,
    pub hostname: String,
    pub os_name: String,
    pub os_version: String,
}

// Resultado del registro: PEM listos para guardar en el disco
#[derive(Debug, Clone)]
pub struct EnrollResult {
    pub cert_pem: String,
    pub private_key: String,
    pub ca: String,
}

// Errores específicos del flujo
#[derive(Debug, thiserror::Error)]
pub enum EnrollError {
    #[error("Error al generar el par de claves: {0}")]
    KeyGeneration(#[from] rcgen::Error),

    #[error("Error de transporte gRPC: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("El servidor rechazó el registro: {0}")]
    ServerRejected(String),

    #[error("url de registro invalida: {0}")]
    InvalidUrl(#[from] tonic::codegen::http::uri::InvalidUri),

    #[error("Error de E/S al guardar los certificados: {0}")]
    Io(#[from] std::io::Error),

    #[error("La respuesta del servidor no contiene certificado")]
    EmptyCertificate,
}

// Genera un par de claves ed25519 y construye el CSR
// Devuelve un `KeyPair` (para extraer la clave privada PEM) y el CSR en formato PEM
fn generate_keypair_and_csr(hostname: &str) -> Result<(KeyPair, String), EnrollError> {
    // Generar par de claves ed25519
    let key_pair = KeyPair::generate_for(&PKCS_ED25519)?;

    // Añadir parámetros del CSR
    let mut params = CertificateParams::default();

    // El CN lo usa el server para identificar al agente
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, hostname);
    dn.push(DnType::OrganizationName, "Complyx Agent");
    params.distinguished_name = dn;

    // SAN con el hostname, para validar TLS
    params.subject_alt_names = vec![SanType::DnsName(hostname.try_into().map_err(|_| {
        rcgen::Error::CouldNotParseCertificate
    })?)];

    // Para generar el CSR en formato PEM
    let csr = params.serialize_request(&key_pair)?;
    let csr_pem = csr.pem()?;

    tracing::debug!(hostname, "par de claves ed25519 y CSR generados");

    Ok((key_pair, csr_pem))
}

// Ejecuta el flujo de registro contra el servidor
// Argumentos
// * `enroll_url`: URL del endpoint de enrolamiento. Ej: https://server:9001
// Este endpoint usa TLS del servidor (sin certificado de cliente), ya que es la primera vez que el
// agente se conecta y aún no tiene certificado.
//
// * `req`: Datos del agente necesarios para el registro.
//
// Errores
// * `EnrollError::KeyGeneration`: Si falla la generación del par de claves
// * `EnrollError::Transport`: Si no se puede conectar al servidor
// * `EnrollError::ServerRejected`: Si el servidor rechaza el token o el CSR
// * `EnrollError::EmptyCertificate`: Si la respuesta del servidor no incluye certificado
pub async fn enroll(enroll_url: &str, req: EnrollRequest) -> Result<EnrollResult, EnrollError> {
    tracing::info!(hostname = %req.hostname, enroll_url, "Iniciando flujo de registro");

    // Generar par de claves y CSR
    let (key_pair, csr_pem) = generate_keypair_and_csr(&req.hostname)?;

    // Conectar al endpoint de registro
    let channel = Channel::from_shared(enroll_url.to_string())?  // ? convierte InvalidUri -> EnrollError::InvalidUrl
        .tls_config(ClientTlsConfig::new().with_enabled_roots())?
        .connect()
        .await?;

    let mut client = ComplyxEnrollClient::new(channel);

    // Enviar solicitud de registro
    let grpc_req = ProtoEnrollRequest {
        token: req.token,
        csr_pem: csr_pem.clone(),
        hostname: req.hostname.clone(),
        os_name: req.os_name.clone(),
        os_version: req.os_version.clone(),
    };

    tracing::debug!("Enviando EnrollRequest al servidor");

    let response = client.enroll(grpc_req)
        .await
        .map_err(|s| EnrollError::ServerRejected(format!("{}: {}", s.code() as i32, s.message())))?
        .into_inner();

    if response.cert_pem.is_empty() {
        return Err(EnrollError::EmptyCertificate);
    }

    if response.ca_cert_pem.is_empty() {
        return Err(EnrollError::EmptyCertificate);
    }

    // Extraer la clave rivada PEM del par de claves
    let private_key_pem = key_pair.serialize_pem();

    tracing::info!(
        hostname = %req.hostname,
        "enrolamiento completado, certificado recibido"
    );
 
    Ok(EnrollResult {
        cert_pem: response.cert_pem,
        // en esta no hace falta response porque está arriba
        private_key: private_key_pem,
        ca: response.ca_cert_pem,
    })
}

// Guarda los certificados del registro en el directorio configurado
// Crea el directorio si no existe. Establece permisos restrictivos para la clave privada
//
// Ficheros que crea
// * `<cert_dir>/agent.crt`: certificado del agente
// * `<cert_dir>/agent.key`: clave privada
// * `<cert_dir>/ca.crt`: certificado de la CA del servidor
pub async fn save_certs(cert_dir: impl AsRef<Path>, result: &EnrollResult) -> Result<(), EnrollError> {
    let dir = cert_dir.as_ref();
    tokio::fs::create_dir_all(dir).await?;

    let cert_path = dir.join("agent.crt");
    let key_path = dir.join("agent.key");
    let ca_path = dir.join("ca.crt");

    tokio::fs::write(&cert_path, result.cert_pem.as_bytes()).await?;
    tokio::fs::write(&key_path, result.private_key.as_bytes()).await?;
    tokio::fs::write(&ca_path, result.ca.as_bytes()).await?;

    // Restringir la privada al propietario
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        tokio::fs::set_permissions(&key_path, perms).await?;
    }

    tracing::info!(cert = %cert_path.display(), key = %key_path.display(), ca = %ca_path.display());

    Ok(())
}

// Pruebas generadas via LLM local fine-tuneado (Qwen-Coder-30B-A3B-Instruct) para reducir tiempo de desarrollo
//#[cfg(test)]
//mod tests {
//    use super::*;
// 
//    #[test]
//    fn generate_keypair_produces_valid_pem() {
//        let (key_pair, csr_pem) = generate_keypair_and_csr("test-agent.local").unwrap();
// 
//        // La clave privada debe ser PEM válido
//        let key_pem = key_pair.serialize_pem();
//        assert!(key_pem.contains("PRIVATE KEY"), "la clave privada debe estar en formato PEM");
// 
//        // El CSR debe ser PEM válido
//        assert!(
//            csr_pem.contains("CERTIFICATE REQUEST"),
//            "el CSR debe estar en formato PEM"
//        );
//    }
// 
//    #[test]
//    fn generate_keypair_uses_hostname_as_cn() {
//        let hostname = "web-01.acme.com";
//        let (_, csr_pem) = generate_keypair_and_csr(hostname).unwrap();
//        // Verificamos que el CSR se generó sin errores para el hostname dado
//        assert!(!csr_pem.is_empty());
//    }
// 
//    #[tokio::test]
//    async fn save_certs_creates_files() {
//        let dir = tempfile::tempdir().unwrap();
//        let result = EnrollResult {
//            cert_pem: "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----\n".into(),
//            private_key_pem: "-----BEGIN PRIVATE KEY-----\ntest\n-----END PRIVATE KEY-----\n".into(),
//            ca_cert_pem: "-----BEGIN CERTIFICATE-----\nca\n-----END CERTIFICATE-----\n".into(),
//        };
// 
//        save_certs(dir.path(), &result).await.unwrap();
// 
//        assert!(dir.path().join("agent.crt").exists());
//        assert!(dir.path().join("agent.key").exists());
//        assert!(dir.path().join("ca.crt").exists());
//    }
// 
//    #[cfg(unix)]
//    #[tokio::test]
//    async fn save_certs_restricts_key_permissions() {
//        use std::os::unix::fs::PermissionsExt;
// 
//        let dir = tempfile::tempdir().unwrap();
//        let result = EnrollResult {
//            cert_pem: "cert".into(),
//            private_key_pem: "key".into(),
//            ca_cert_pem: "ca".into(),
//        };
// 
//        save_certs(dir.path(), &result).await.unwrap();
// 
//        let meta = std::fs::metadata(dir.path().join("agent.key")).unwrap();
//        let mode = meta.permissions().mode() & 0o777;
//        assert_eq!(mode, 0o600, "la clave privada debe tener permisos 0o600");
//    }
//}
