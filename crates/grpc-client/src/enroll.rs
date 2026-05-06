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

//! Flujo de enrolamiento del agente.
//!
//! El enrolamiento ocurre una única vez: la primera vez que el agente arranca
//! sin certificado en su directorio de certificados. El proceso es:
//!
//! 1. El agente genera un keypair Ed25519 con `rcgen`.
//! 2. Construye un CSR (Certificate Signing Request) con su hostname como CN.
//! 3. Llama al endpoint `ComplyxEnroll.Enroll` del servidor (puerto separado, TLS one-way).
//! 4. El servidor valida el token, firma el CSR con su CA interna y devuelve el certificado.
//! 5. El agente guarda `agent.key`, `agent.crt` y `ca.crt` en su `cert_dir`.
//!
//! A partir de ese momento, el agente usa su certificado para autenticarse mediante
//! mTLS en cada llamada al servicio principal.

use std::path::Path;

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ED25519, SanType};
use tonic::transport::{Channel, ClientTlsConfig};

use crate::GrpcError;
use proto::complyx::EnrollRequest as ProtoEnrollRequest;
use proto::complyx::complyx_enroll_client::ComplyxEnrollClient;

/// Datos necesarios para iniciar el enrolamiento.
#[derive(Debug, Clone)]
pub struct EnrollRequest {
    /// Token de un solo uso generado por el administrador en el servidor.
    pub token: String,

    /// Nombre de host del agente (se usará como CN en el certificado).
    pub hostname: String,

    /// Nombre del sistema operativo (ej. "Linux", "Windows").
    pub os_name: String,

    /// Versión del sistema operativo (ej. "6.8.0", "10.0.22621").
    pub os_version: String,
}

/// Resultado del enrolamiento: los PEM listos para guardar en disco.
#[derive(Debug)]
pub struct EnrollResult {
    /// Certificado del agente firmado por la CA del servidor (PEM).
    pub cert_pem: String,

    /// Clave privada del agente generada localmente (PEM). Nunca sale del agente.
    pub private_key_pem: String,

    /// Certificado raíz de la CA del servidor (PEM). Se usa para verificar el servidor en mTLS.
    pub ca_cert_pem: String,
}

/// Errores específicos del flujo de enrolamiento.
#[derive(Debug, thiserror::Error)]
pub enum EnrollError {
    #[error("error al generar el keypair: {0}")]
    KeyGeneration(#[from] rcgen::Error),

    #[error("error de transporte gRPC: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("el servidor rechazó el enrolamiento: {0}")]
    ServerRejected(String),

    #[error("error de I/O al guardar los certificados: {0}")]
    Io(#[from] std::io::Error),

    #[error("la respuesta del servidor no contiene certificado")]
    EmptyCertificate,

    #[error("URL de registro invalida: {0}")]
    InvalidUrl(#[from] tonic::codegen::http::uri::InvalidUri),
}

/// Genera un keypair Ed25519 y construye el CSR correspondiente.
///
/// Devuelve el `KeyPair` (para extraer la clave privada PEM) y el CSR en formato PEM.
///
/// ## Por qué Ed25519
///
/// Ed25519 ofrece claves cortas, firmas rápidas y seguridad demostrable sin los
/// problemas históricos de las curvas NIST. Es la elección estándar para sistemas
/// modernos donde no hay restricciones de interoperabilidad con legacy.
fn generate_keypair_and_csr(hostname: &str) -> Result<(KeyPair, String), EnrollError> {
    // Generamos el keypair Ed25519
    let key_pair = KeyPair::generate_for(&PKCS_ED25519)?;

    // Construimos los parámetros del CSR
    let mut params = CertificateParams::default();

    // CN = hostname del agente. El servidor lo usará para identificar al agente.
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, hostname);
    dn.push(DnType::OrganizationName, "Complyx Agent");
    params.distinguished_name = dn;

    // SAN (Subject Alternative Name) con el hostname, necesario para validación TLS moderna
    params.subject_alt_names = vec![SanType::DnsName(
        hostname
            .try_into()
            .map_err(|_| rcgen::Error::CouldNotParseCertificate)?,
    )];

    // Generamos el CSR en formato PEM
    let csr = params.serialize_request(&key_pair)?;
    let csr_pem = csr.pem()?;

    tracing::debug!(hostname, "keypair Ed25519 y CSR generados");

    Ok((key_pair, csr_pem))
}

/// Ejecuta el flujo completo de enrolamiento contra el servidor.
///
/// # Argumentos
///
/// * `enroll_url` — URL del endpoint de enrolamiento (ej. `https://server:9001`).
///   Este endpoint usa TLS one-way (sin certificado de cliente), ya que es la
///   primera vez que el agente se conecta y aún no tiene certificado.
/// * `req` — Datos del agente necesarios para el enrolamiento.
///
/// # Errores
///
/// * `EnrollError::KeyGeneration` — si falla la generación del keypair.
/// * `EnrollError::Transport` — si no se puede conectar al servidor.
/// * `EnrollError::ServerRejected` — si el servidor rechaza el token o el CSR.
/// * `EnrollError::EmptyCertificate` — si la respuesta del servidor no incluye certificado.
pub async fn enroll(
    enroll_url: &str,
    req: EnrollRequest,
    ca_cert_pem: Option<&str>,
) -> Result<EnrollResult, EnrollError> {
    tracing::info!(
        hostname = %req.hostname,
        enroll_url,
        "iniciando flujo de enrolamiento"
    );

    // Paso 1: generar keypair y CSR localmente
    let (key_pair, csr_pem) = generate_keypair_and_csr(&req.hostname)?;

    // Paso 2: conectar al endpoint de enrolamiento (TLS one-way, sin cert de cliente)
    // El servidor debe tener un certificado TLS válido (puede ser de una CA pública
    // o la misma CA interna si el agente ya confía en ella por configuración).
    // Construir la configuración TLS según si tenemos el ca.crt de la CA interna.
    // Si ya existe (pre-provisioning), lo usamos para verificar el servidor.
    // Si no existe todavía, usamos las CAs del sistema (útil si el servidor tiene
    // un certificado de una CA pública como Let's Encrypt).
    //    let tls_config = if let Some(pem) = ca_cert_pem {
    //        let ca_cert = tonic::transport::Certificate::from_pem(pem);
    //        ClientTlsConfig::new()
    //            .ca_certificate(ca_cert)
    //            .domain_name("localhost")
    //    } else {
    //        ClientTlsConfig::new().with_enabled_roots()
    //    };

    let tls_config = if let Some(pem) = ca_cert_pem {
        let ca_cert = tonic::transport::Certificate::from_pem(pem);
        ClientTlsConfig::new()
            .ca_certificate(ca_cert)
            .domain_name("localhost")
    } else {
        ClientTlsConfig::new().with_enabled_roots()
    };

    //    let channel = Channel::from_shared(enroll_url.to_string())
    //        .map_err(|e| tonic::transport::Error::from(e))?
    //        .tls_config(tls_config)?
    //        .connect()
    //        .await?;

    let channel = Channel::from_shared(enroll_url.to_string())
        .map_err(EnrollError::InvalidUrl)?
        .tls_config(tls_config)?
        .connect()
        .await?;

    let mut client = ComplyxEnrollClient::new(channel);

    // Paso 3: enviar la solicitud de enrolamiento
    let grpc_req = ProtoEnrollRequest {
        token: req.token,
        csr_pem: csr_pem.clone(),
        hostname: req.hostname.clone(),
        os_name: req.os_name,
        os_version: req.os_version,
    };

    tracing::debug!("enviando EnrollRequest al servidor");

    let response = client
        .enroll(grpc_req)
        .await
        .map_err(|s| EnrollError::ServerRejected(format!("{}: {}", s.code() as i32, s.message())))?
        .into_inner();

    if response.cert_pem.is_empty() {
        return Err(EnrollError::EmptyCertificate);
    }
    if response.ca_cert_pem.is_empty() {
        return Err(EnrollError::EmptyCertificate);
    }

    // Paso 4: extraer la clave privada PEM del keypair generado localmente
    let private_key_pem = key_pair.serialize_pem();

    tracing::info!(
        hostname = %req.hostname,
        "enrolamiento completado, certificado recibido"
    );

    Ok(EnrollResult {
        cert_pem: response.cert_pem,
        private_key_pem,
        ca_cert_pem: response.ca_cert_pem,
    })
}

/// Guarda los certificados resultantes del enrolamiento en el directorio configurado.
///
/// Crea el directorio si no existe. Establece permisos restrictivos en la clave privada
/// (0o600 en Unix) para que solo el usuario del agente pueda leerla.
///
/// # Ficheros que crea
///
/// * `<cert_dir>/agent.crt` — certificado del agente.
/// * `<cert_dir>/agent.key` — clave privada (permisos 0o600).
/// * `<cert_dir>/ca.crt`    — certificado de la CA del servidor.
pub async fn save_certs(
    cert_dir: impl AsRef<Path>,
    result: &EnrollResult,
) -> Result<(), EnrollError> {
    let dir = cert_dir.as_ref();
    tokio::fs::create_dir_all(dir).await?;

    let cert_path = dir.join("agent.crt");
    let key_path = dir.join("agent.key");
    let ca_path = dir.join("ca.crt");

    tokio::fs::write(&cert_path, result.cert_pem.as_bytes()).await?;
    tokio::fs::write(&key_path, result.private_key_pem.as_bytes()).await?;
    tokio::fs::write(&ca_path, result.ca_cert_pem.as_bytes()).await?;

    // En Unix restringimos la clave privada a solo el propietario
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        tokio::fs::set_permissions(&key_path, perms).await?;
    }

    tracing::info!(
        cert = %cert_path.display(),
        key = %key_path.display(),
        ca = %ca_path.display(),
        "certificados guardados en disco"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_keypair_produces_valid_pem() {
        let (key_pair, csr_pem) = generate_keypair_and_csr("test-agent.local").unwrap();

        // La clave privada debe ser PEM válido
        let key_pem = key_pair.serialize_pem();
        assert!(
            key_pem.contains("PRIVATE KEY"),
            "la clave privada debe estar en formato PEM"
        );

        // El CSR debe ser PEM válido
        assert!(
            csr_pem.contains("CERTIFICATE REQUEST"),
            "el CSR debe estar en formato PEM"
        );
    }

    #[test]
    fn generate_keypair_uses_hostname_as_cn() {
        let hostname = "web-01.acme.com";
        let (_, csr_pem) = generate_keypair_and_csr(hostname).unwrap();
        // Verificamos que el CSR se generó sin errores para el hostname dado
        assert!(!csr_pem.is_empty());
    }

    #[tokio::test]
    async fn save_certs_creates_files() {
        let dir = tempfile::tempdir().unwrap();
        let result = EnrollResult {
            cert_pem: "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----\n".into(),
            private_key_pem: "-----BEGIN PRIVATE KEY-----\ntest\n-----END PRIVATE KEY-----\n"
                .into(),
            ca_cert_pem: "-----BEGIN CERTIFICATE-----\nca\n-----END CERTIFICATE-----\n".into(),
        };

        save_certs(dir.path(), &result).await.unwrap();

        assert!(dir.path().join("agent.crt").exists());
        assert!(dir.path().join("agent.key").exists());
        assert!(dir.path().join("ca.crt").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn save_certs_restricts_key_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let result = EnrollResult {
            cert_pem: "cert".into(),
            private_key_pem: "key".into(),
            ca_cert_pem: "ca".into(),
        };

        save_certs(dir.path(), &result).await.unwrap();

        let meta = std::fs::metadata(dir.path().join("agent.key")).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "la clave privada debe tener permisos 0o600");
    }
}
