//! Punto de entrada del agente Complyx.
//!
//! Secuencia de arranque:
//!
//! 1. Cargar y validar configuración.
//! 2. Inicializar telemetría (tracing).
//! 3. Conectar a la BD local SQLite (crea el fichero si no existe).
//! 4. Recuperar resultados `sending` que quedaron a medias en el arranque anterior.
//! 5. Si el agente no está enrolado → ejecutar flujo de enrolamiento.
//! 6. Conectar el cliente gRPC (lazy — no falla si el servidor no está disponible).
//! 7. Inicializar policy-engine y remediation-engine.
//! 8. Arrancar el scheduler con gestión de señales SIGTERM/SIGINT.

//! Punto de entrada del agente Complyx.
//!
//! Secuencia de arranque:
//!
//! 1. Cargar y validar configuración.
//! 2. Inicializar telemetría (tracing).
//! 3. Conectar a la BD local SQLite (crea el fichero si no existe).
//! 4. Recuperar resultados `sending` que quedaron a medias en el arranque anterior.
//! 5. Si el agente no está enrolado → ejecutar flujo de enrolamiento.
//! 6. Conectar el cliente gRPC (lazy — no falla si el servidor no está disponible).
//! 7. Inicializar policy-engine y remediation-engine.
//! 8. Arrancar el scheduler con gestión de señales SIGTERM/SIGINT.

mod config;
mod poll_loop;
mod scheduler;

use std::sync::Arc;

use grpc_client::{
    GrpcClient, GrpcClientConfig,
    enroll::{self, EnrollRequest},
};
use local_db::LocalDb;
use policy_engine::PolicyEngine;
use remediation_engine::RemediationEngine;
use tracing_subscriber::EnvFilter;

use config::AgentConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // -----------------------------------------------------------------------
    // 1. Configuración
    // -----------------------------------------------------------------------
    let config = config::load(None)?;
    config.validate()?;

    // -----------------------------------------------------------------------
    // 2. Telemetría (antes de cualquier log)
    // -----------------------------------------------------------------------
    init_tracing(&config);

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        agent_id = %config.agent_id,
        server_url = %config.server_url,
        "complyx-agent arrancando"
    );

    // -----------------------------------------------------------------------
    // 3. Base de datos local
    // -----------------------------------------------------------------------
    let db = LocalDb::connect(&config.db_path).await.map_err(|e| {
        anyhow::anyhow!(
            "no se pudo abrir la BD local en {:?}: {}",
            config.db_path,
            e
        )
    })?;

    tracing::info!(db_path = ?config.db_path, "BD local inicializada");

    // -----------------------------------------------------------------------
    // 4. Recuperar resultados interrupted en el arranque anterior
    // -----------------------------------------------------------------------
    let recovered = db.reset_sending_to_pending().await?;
    if recovered > 0 {
        tracing::warn!(
            count = recovered,
            "resultados en estado 'sending' recuperados a 'pending'"
        );
    }

    // Detectar remediaciones incompletas del arranque anterior
    let pending_audits = db.get_pending_audits().await?;
    if !pending_audits.is_empty() {
        tracing::warn!(
            count = pending_audits.len(),
            "remediaciones incompletas detectadas del arranque anterior"
        );
        for audit in &pending_audits {
            tracing::warn!(
                audit_id = %audit.id,
                check_id = %audit.check_id,
                remediation_type = %audit.remediation_type,
                started_at = %audit.started_at,
                "remediación incompleta"
            );
        }
    }

    let config = Arc::new(config);

    // -----------------------------------------------------------------------
    // 5. Enrolamiento (si no está enrolado)
    // -----------------------------------------------------------------------
    if !config.is_enrolled() {
        tracing::info!("agente no enrolado, iniciando flujo de enrolamiento");
        run_enrollment(&config).await?;
        tracing::info!("enrolamiento completado");
    } else {
        tracing::info!(
            cert_dir = ?config.cert_dir,
            "agente ya enrolado, usando certificados existentes"
        );
    }

    // -----------------------------------------------------------------------
    // 6. Cliente gRPC
    // -----------------------------------------------------------------------
    let client = GrpcClient::connect(GrpcClientConfig {
        server_url: config.server_url.clone(),
        cert_dir: config.cert_dir.clone(),
        agent_id: config.agent_id.clone(),
    })
    .await
    .map_err(|e| anyhow::anyhow!("no se pudo inicializar el cliente gRPC: {}", e))?;

    tracing::info!(server_url = %config.server_url, "cliente gRPC inicializado");

    // -----------------------------------------------------------------------
    // 7. Engines
    // -----------------------------------------------------------------------
    let engine = PolicyEngine::new();
    tracing::info!(
        types = ?engine.supported_check_types(),
        "policy-engine inicializado"
    );

    let remediation = RemediationEngine::new(db.clone());
    tracing::info!(
        types = ?remediation.supported_types(),
        "remediation-engine inicializado"
    );

    // Pending count al arrancar (informativo)
    let pending = db.pending_count().await.unwrap_or(0);
    if pending > 0 {
        tracing::info!(pending, "resultados pendientes de envío en la cola local");
    }

    // -----------------------------------------------------------------------
    // 8. Señales de parada + scheduler
    // -----------------------------------------------------------------------
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Capturar SIGTERM y SIGINT para shutdown graceful
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        tracing::info!("señal de parada recibida");
        let _ = shutdown_tx_clone.send(true);
    });

    tracing::info!("scheduler arrancando");

    scheduler::run(
        Arc::clone(&config),
        Arc::new(client),
        Arc::new(db),
        Arc::new(engine),
        Arc::new(remediation),
        shutdown_rx,
    )
    .await;

    tracing::info!("complyx-agent finalizado correctamente");
    Ok(())
}

/// Ejecuta el flujo de enrolamiento completo.
///
/// Lee el token del campo `enroll_token` de la configuración o de la variable
/// de entorno `COMPLYX_ENROLL_TOKEN`.
async fn run_enrollment(config: &AgentConfig) -> anyhow::Result<()> {
    let token = config
        .enroll_token
        .clone()
        .or_else(|| std::env::var("COMPLYX_ENROLL_TOKEN").ok())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "el agente no está enrolado y no se encontró COMPLYX_ENROLL_TOKEN. \
             Genera un token con 'complyx-server enroll-token' y ejecútalo como: \
             COMPLYX_ENROLL_TOKEN=<token> complyx-agent"
            )
        })?;

    let hostname = std::fs::read_to_string("/etc/hostname")
        .map(|h| h.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let os_name = std::env::consts::OS.to_string();
    let os_version = read_os_version();

    tracing::info!(hostname = %hostname, os_name = %os_name, "iniciando enrolamiento");

    let req = EnrollRequest {
        token,
        hostname,
        os_name,
        os_version,
    };

    // Leer el ca.crt si ya existe en el cert_dir (pre-provisioning).
    // Si no existe todavía, pasamos None y se usarán las CAs del sistema.
    let ca_cert_pem = {
        let ca_path = config.cert_dir.join("ca.crt");
        match tokio::fs::read_to_string(&ca_path).await {
            Ok(pem) => {
                tracing::info!(path = %ca_path.display(), "usando ca.crt pre-provisionado para el enrolamiento");
                Some(pem)
            }
            Err(_) => {
                tracing::warn!(
                    "ca.crt no encontrado en {:?}, usando CAs del sistema para verificar el servidor",
                    config.cert_dir
                );
                None
            }
        }
    };

    let result = enroll::enroll(&config.enroll_url, req, ca_cert_pem.as_deref())
        .await
        .map_err(|e| anyhow::anyhow!("enrolamiento fallido: {}", e))?;

    enroll::save_certs(&config.cert_dir, &result)
        .await
        .map_err(|e| anyhow::anyhow!("no se pudieron guardar los certificados: {}", e))?;

    tracing::info!(cert_dir = ?config.cert_dir, "certificados guardados");

    Ok(())
}

/// Inicializa el sistema de logging estructurado.
fn init_tracing(config: &AgentConfig) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log_level));

    match config.log_format {
        config::LogFormat::Json => {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(filter)
                .with_current_span(true)
                .with_span_list(false)
                .init();
        }
        config::LogFormat::Pretty => {
            tracing_subscriber::fmt()
                .pretty()
                .with_env_filter(filter)
                .init();
        }
    }
}

/// Lee la versión del sistema operativo de `/etc/os-release`.
fn read_os_version() -> String {
    std::fs::read_to_string("/etc/os-release")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("VERSION_ID="))
        .and_then(|l| l.strip_prefix("VERSION_ID="))
        .map(|v| v.trim_matches('"').to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Espera SIGTERM o SIGINT (Ctrl+C).
async fn wait_for_shutdown_signal() {
    use tokio::signal;

    #[cfg(unix)]
    {
        use signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).expect("no se pudo registrar SIGTERM");
        let mut sigint = signal(SignalKind::interrupt()).expect("no se pudo registrar SIGINT");

        tokio::select! {
            _ = sigterm.recv() => tracing::debug!("SIGTERM recibido"),
            _ = sigint.recv()  => tracing::debug!("SIGINT recibido"),
        }
    }

    #[cfg(not(unix))]
    {
        signal::ctrl_c().await.expect("no se pudo registrar Ctrl+C");
    }
}
