//! Cliente gRPC principal del agente
//!
//! `GrpcClient` es el punto de entrada para la comunicación con el servidor
//! Complyx tras el registro, encapsula:
//! * La conexión gRPC con mTLS.
//! * La reconexión automática ante caídas del servidor
//! * La política de reintentos para llamadas transitorias
//! * Traducción dentre tipos proto y tipos del dominio del agente
//!
//! Diseño de la reconexión
//! Tonico con HTTP/2 gestiona la reconexión de la capa de transporte de forma automatica mediante
//! `connect_lazy`. Sin embargo, si la conexión falla durante el handshake TLS, Tonic no puede
//! recuperarla por sí solo. `GrpcClient` detecta estos casos y reconstruye el `Channel` completo
//! desde zero.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use tonic::transport::Channel;

use crate::mtls::CertPaths;
use crate::retry::RetryPolicy;
use crate::GrpcError;
use proto::complyx::complyx_agent_client::ComplyxAgentClient;
use proto::complyx::{CheckResult as ProtoCheckResult, HeartbeatRequest, PollRequest, PollResponse, SubmitResultsRequest, SubmitResultsResponse};

// Configuracion del cliente gRPC
#[derive(Debug, Clone)]
pub struct GrpcClientConfig {
    pub server_url: String,
    pub cert_dir: PathBuf,
    pub agent_id: String
}

// Estado interno compartido entre cliente y tareas de reconexion
struct ClientState {
    // Canal gRPC activo. None si la conexion no se ha establecido o el canal anterior fallo y esta
    // esperando reconexion
    channel: Option<Channel>,
}

// cliente gRPC del agente
//
// Al usar Arc para compartir el estado, no debería de costar mucho clonar. Clonar el cliente da
// acceso al mismo canal gRPC subyacente
#[derive(Clone)]
pub struct GrpcClient {
    config: GrpcClientConfig,
    state: Arc<Mutex<ClientState>>,
    retry_policy: RetryPolicy,
}

impl GrpcClient {
    // Crea el cliente y establece la conexion inicial con el servidor
    // Usa `connect_lazy` de Tonic: la conexion TCP + TLS se establece en la primera llamada, no en
    // este momento para evitar que el agente falle al arrancar si el servidor tarda un momento en
    // estar disponible.
    //
    // Errores
    // * devuelve `GrpcError::Tls` si los certificados no existen o no son validos
    // * Devuelve `GrpcError::Io` si no se pueden leer los ficheros de certificado
    
    pub async fn connect(config: GrpcClientConfig) -> Result<Self, GrpcError> {
        let cert_paths = CertPaths::from_dir(&config.cert_dir);
        let tls_config = crate::mtls::build_tls_config(&cert_paths).await?;

//        let channel = Channel::from_shared(config.server_url.clone())
//            .map_err(|e| {
//                // uri invalida. Convertir al error de transporte de tonic
//                GrpcError::ConnectionFailed {attempts: 1, source: e, }
//            })
//            .and_then(|b| Ok(b.tls_config(tls_config)?))?
//            .connect_lazy();

//        let channel = Channel::from_shared(config.server_url.clone())
//            .map_err(|e| GrpcError::Transport(e.into()))?
//            .tls_config(tls_config)
//            .map_err(GrpcError::Transport)?
//            .connect_lazy();

        let channel = Channel::from_shared(config.server_url.clone())? // ? convierte InvalidUri en
        // GrpcError::InvalidUrl
            .tls_config(tls_config)
            .map_err(GrpcError::Transport)?
            .connect_lazy();

        tracing::info!(
            server_url = %config.server_url,
            agent_id = %config.agent_id,
            "cliente gRPC inicializaco (conexión lazy)"
            );

        Ok(Self {
            config,
            state: Arc::new(Mutex::new(ClientState {
                channel: Some(channel),
            })),
            retry_policy: RetryPolicy::for_poll(),
        })
    }

    // Configura la politica de reintentos del cliente.
    // Util para usar `RetryPolicy::for_startup()` en el arranque del agente

    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    // Parte publica

    // Hace poll al servidor para obtener el bundle de politicas asignadas a este agente.
    // Envia el hash del bundle que el agente tiene actualmente en caché. Si el servidor detecta que
    // no ha cambiado, devuelve `PollResponse { policies_changed: false }` y el campo `bundle` está
    // vacío, ahorrando ancho de banda.
    //
    // Argumentos:
    // * `current_bundle_hash`: hash SHA-256 del `PolicyBundle` almacenado en la base de datos local
    // del agente. Cadena vacía si no hay bundle cacheado.
    //
    // Errores
    // * Devuelve `GrpcError::Status` si el servidor responde con error.
    // * Devuelve `GrpcError::ConnectionFailed` si se agotan los reintentos de conexion

    pub async fn poll_policies(&self, current_bundle_hash: &str) -> Result<PollResponse, GrpcError> {
        let agent_id = self.config.agent_id.clone();
        let hash = current_bundle_hash.to_string();

        tracing::debug!(
            agent_id = %agent_id,
            current_hash = %hash,
            "Enviando PollRequest"
            );

//        let response = self
//            .with_retry_policy(|mut client| {
//                let req = PollRequest {
//                    agent_id: agent_id.clone(),
//                    policy_bundle_hash: hash.clone(),
//                };
//                async move {
//                    client
//                        .poll_policies(req)
//                        .await
//                        .map(|r| r.into_inner())
//                }
//            })?;

        let response = self
            .with_retry(|mut client| {
                let req = PollRequest {
                    agent_id: agent_id.clone(),
                    policy_bundle_hash: hash.clone(),
                };

                async move {
                    client.poll_policies(req).await.map(|r| r.into_inner())
                }
            })
            .await?;

        tracing::debug!(
            policies_changed = response.policies_changed,
            bundle_hash = response.bundle.as_ref().map(|b| b.bundle_hash.as_str()).unwrap_or(""),
            "PollResponse recibido"
            );

        Ok(response)
    }

    // Envia los resultados de los check al servidor
    //
    // Los resultados se envian en un solo request. Si la red no esta disponible, el caller
    // (poll_loop) debe encolar los resultados en `local-db` y reintentar en el siguiente tick del
    // `result_flush`.
    //
    // Argumentos
    // * `results`: lista de resultados de checks ejecutados por el `policy-engine`.
    //
    // Errores
    // * Devuelve `GrpcError` si el servidor rechaza los resultados o hay error de red.
    //
    pub async fn submit_results(&self, results: Vec<ProtoCheckResult>) -> Result<SubmitResultsResponse, GrpcError> {
        let agent_id = self.config.agent_id.clone();
        let count = results.len();

        tracing::debug!(
            agent_id = %agent_id,
            result_count = count,
            "Enviando SubmitResultsRequest"
            );

        let response = self.with_retry(|mut client| {
            let req = SubmitResultsRequest {
                agent_id: agent_id.clone(),
                results: results.clone(),
            };

            async move {
                client.
                    submit_results(req)
                    .await
                    .map(|r| r.into_inner())
            }
        }).await?;

        if response.accepted {
            tracing::info!(result_count = count, "resultados aceptados por el servidor");
        } else {
            tracing::warn!(result_count = count, "El servidor no aceptó los resultados");
        }

        Ok(response)
    }

    // Envía un hartbeat (ping) para que el orchestrator actualice `last_seen`.
    // Se llama desde `scheduler.rs` de forma independiente al poll de politicas.
    // En la práctica, el poll ya actualiza el hartbeat en el servidor, pero esta llamada explicita
    // sirve para agentes que esten en modo solo-heartbeat (sin políticas asignadas, vaya).
    pub async fn heartbeat(&self) -> Result<(), GrpcError> {
        let agent_id = self.config.agent_id.clone();

        self.with_retry(|mut client| {
            let req = HeartbeatRequest {
                agent_id: agent_id.clone(),
                timestamp: chrono_timestamp(),
            };

            async move {
                client
                    .heartbeat(req)
                    .await
                    .map(|_| ())
            }
        }).await?;

        tracing::debug!(agent_id = %agent_id, "heartbeat enviado");

        Ok(())
    }

    // Infraestructura interna

    // Ejecuta una llamada gRPC con la politica de reintentos configurada.
    // `f` recibe un `ComplyxAgentClient<Channel>` listo para usar. Si la llamada falla en un error
    // retryable, se reintenta obteniendo un cliente fresco (que puede haber reeconectado)
    async fn with_retry<F, Fut, T>(&self, mut f: F) -> Result<T, GrpcError>
    where
        F: FnMut(ComplyxAgentClient<Channel>) -> Fut,
        Fut: Future<Output = Result<T, tonic::Status>>,
    {
        let policy = self.retry_policy.clone();
        let state = Arc::clone(&self.state);
        let config = self.config.clone();
        let mut last_error: Option<tonic::Status> = None;
    
        for attempt in 0..policy.max_attempts {
            let client = match Self::get_or_reconnect(&state, &config).await {
                Ok(c) => c,
                Err(s) => {
                    last_error = Some(s);
                    break;
                }
            };
    
            match f(client).await {
                Ok(value) => {
                    if attempt > 0 {
                        tracing::info!(attempt, "llamada gRPC exitosa tras reintento");
                    }
                    return Ok(value);
                }
                Err(status) if !RetryPolicy::is_retryable(status.code()) => {
                    return Err(GrpcError::from(status));
                }
                Err(status) => {
                    let remaining = policy.max_attempts - attempt - 1;
                    if remaining == 0 {
                        last_error = Some(status);
                        break;
                    }
                    let delay = policy.delay_for_attempt(attempt);
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_attempts = policy.max_attempts,
                        delay_ms = delay.as_millis(),
                        code = ?status.code(),
                        "error gRPC transitorio, reintentando..."
                    );
                    tokio::time::sleep(delay).await;
                    last_error = Some(status);
                }
            }
        }

        Err(GrpcError::from(last_error.unwrap_or_else(|| {
            tonic::Status::internal("reintentos agotados sin error registrado")
        })))
    }

    // Obtiene el cliente del canal activo, o lo reconstruye si hace falta.
    // Tonic con `connect_lazy` gestiona la reconexión TCP de forma transparente.
    // Esta funcion reconstruye el canal completo solo cuando el canal ha sido marcado
    // explicitamente como invalido. Ej: tras un error de handshake TLS
    async fn get_or_reconnect(state: &Mutex<ClientState>, config: &GrpcClientConfig) -> Result<ComplyxAgentClient<Channel>, tonic::Status> {
        let mut guard = state.lock().await;

        // Si hay canal, lo usamos y listo
        if let Some(channel) = &guard.channel {
            return Ok(ComplyxAgentClient::new(channel.clone()));
        }

        // Reconectar si el canal es invalido
        tracing::info!(
            server_url = %config.server_url,
            "reconectando al servidor gRPC"
            );

        let cert_paths = CertPaths::from_dir(&config.cert_dir);
        let tls_config = crate::mtls::build_tls_config(&cert_paths)
            .await
            .map_err(|e| {
                tonic::Status::internal(format!("Error reconstruyendo TLS: {e}"))
            })?;

        let channel = Channel::from_shared(config.server_url.clone())
            .map_err(|e| tonic::Status::internal(e.to_string()))?
            .tls_config(tls_config)
            .map_err(|e| tonic::Status::internal(e.to_string()))?
            .connect_lazy();

        guard.channel = Some(channel.clone());

        tracing::info!("canal gRPC reconstruido correctamente");

        Ok(ComplyxAgentClient::new(channel))
    }

    // Invalida el canal activo forzando reconexion en la siguiente llamada.
    //
    // Se llama internamente cuando se detecta un error de handshake TLS o cuando los certificados
    // del agente han sido renovados.
    #[allow(dead_code)]
    async fn invalidate_channel(&self) {
        let mut guard = self.state.lock().await;
        guard.channel = None;
        tracing::warn!("canal gRPC invalidado, se reconectará en la siguiente llamada");
    }
}

// Devuelve el timestamp unix actual en segundos
fn chrono_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}


#[cfg(test)]
mod tests {
    use super::*;
 
    #[test]
    fn grpc_client_config_is_cloneable() {
        let config = GrpcClientConfig {
            server_url: "https://localhost:9000".into(),
            cert_dir: PathBuf::from("/var/lib/complyx/certs"),
            agent_id: "550e8400-e29b-41d4-a716-446655440000".into(),
        };
        let cloned = config.clone();
        assert_eq!(config.server_url, cloned.server_url);
        assert_eq!(config.agent_id, cloned.agent_id);
    }
 
    #[test]
    fn chrono_timestamp_is_reasonable() {
        let ts = chrono_timestamp();
        // Debe ser mayor que 2024-01-01 (1704067200) y menor que 2099 (4070908800)
        assert!(ts > 1_704_067_200, "timestamp demasiado antiguo: {ts}");
        assert!(ts < 4_070_908_800, "timestamp demasiado futuro: {ts}");
    }
 
    // Los tests de integración del cliente requieren un servidor gRPC de prueba.
    // Se encuentran en tests/integration_test.rs del crate agent-core.
}
 

