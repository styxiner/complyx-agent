//! Configuración del agente Complyx.
//!
//! La configuración se carga en capas con este orden de precedencia
//! (cada capa sobreescribe la anterior):
//!
//! 1. Valores por defecto compilados.
//! 2. Fichero TOML: `/etc/complyx/agent.toml` (o `COMPLYX_CONFIG_PATH`).
//! 3. Variables de entorno con prefijo `COMPLYX_` (ej. `COMPLYX_SERVER_URL`).
//!
//! Las variables de entorno mapean directamente a los campos del struct:
//! `COMPLYX_SERVER_URL` → `server_url`, `COMPLYX_AGENT_ID` → `agent_id`, etc.

use std::path::PathBuf;

use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};

/// Ruta por defecto al fichero de configuración del agente.
/// Se puede sobreescribir con la variable `COMPLYX_CONFIG_PATH`.
pub const DEFAULT_CONFIG_PATH: &str = "/etc/complyx/agent.toml";

/// Configuración completa del agente.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub agent_id: String, // UUID del agente tal como se registra en el servidor. Se genera durante
                          // el registro y persiste aquí. Vacio antes del primer registro.

    pub server_url: String, // URL del servicio gRPC principal con mTLS. Ej:
                            // https://server.complyx.local:9000

    pub enroll_url: String, // URL del endpoint de registro (TLS solo en el server). Ej: 
                            // https://server.complyx.local:9001

    #[serde(default)]
    pub enroll_token: Option<String>, // Token de un solo uso para el registro inicial. Se lee de
                                      // la variable de entorno `COMPLYX_ENROLL_TOKEN` en el primer
                                      // arranque. Se limpia de la configuracion tras el rergistro.

    #[serde(default = "default_cert_dir")]
    pub cert_dir: PathBuf, // Directorio donde se almacenan los certificados del agente.

    #[serde(default = "default_db_path")]
    pub db_path: PathBuf, // Ruta a la base de datos SQLite local del agente

    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64, // Intervalo en segundo entre polls al servidor para obtener
                                 // politicas.

    /// Intervalo en segundos entre intentos de flush de la cola de resultados.
    #[serde(default = "default_flush_interval")]
    pub flush_interval_secs: u64, // Intervalo en segundos entre intentos de flush de la cola de
                                  // resultados

    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval_secs: u64, // Intervalo en segundos entre heartbeats independientes. El
                                      // poll ya actualiza el heartbeat , pero ese timer sirve para
                                      // agentes sin politicas asignadas.

    #[serde(default = "default_purge_days")]
    pub result_purge_days: i64, // Cuantos dias conservar los resultados ya enviados en la cola
                                // local.

    #[serde(default = "default_true")]
    pub auto_remediate: bool, // Si es falso, el agente ejecuta checks pero nunca las
                              // remediaciones. Mola para auditorias o entornos en los que los
                              // cambios requieran aprobacion 

    // Nivel de log: `error`, `warn`, `info`, `debug`, `trace`.
    #[serde(default = "default_log_level")]
    pub log_level: String, // Nivel de log: `error`, `warn`, `info`, `debug` y `trace`

    #[serde(default = "default_log_format")]
    pub log_format: LogFormat, // Formato del log: `prety` o `json`
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Pretty,
    Json,
}

// Config de valores x defecto
fn default_cert_dir() -> PathBuf {
    PathBuf::from("/var/lib/complyx/certs")
}

fn default_db_path() -> PathBuf {
    PathBuf::from("/var/lib/complyx/agent.db")
}

fn default_poll_interval() -> u64 { 
    300 // 5 mins
}

fn default_flush_interval() -> u64 { 
    60 // 1 min
}

fn default_heartbeat_interval() -> u64 { 
    120 // 2 mins
}

fn default_purge_days() -> i64 { 
    7 
}

fn default_true() -> bool { 
    true 
}

fn default_log_level() -> String { 
    "info".to_string() 
}

fn default_log_format() -> LogFormat { 
    LogFormat::Json 
}


// Carga la configuración del agente aplicando las capas en orden de precedencia.
//
// Podría fallar con error si:
// - El fichero de configuración existe pero contiene TOML inválido.
// - Faltan campos obligatorios (`server_url`, `enroll_url`) en todas las capas.
pub fn load(config_path: Option<&str>) -> anyhow::Result<AgentConfig> {
    let path = config_path
        .unwrap_or(DEFAULT_CONFIG_PATH)
        .to_string();

    // Leer COMPLYX_CONFIG_PATH de las variables de entorno si se especificó
    let path = std::env::var("COMPLYX_CONFIG_PATH").unwrap_or(path);

    let config: AgentConfig = Figment::new()
        // Capa 1: valores por defecto del struct
        .merge(Serialized::defaults(AgentConfig::default_values()))
        // Capa 2: fichero TOML (opcional — puede no existir antes del enrolamiento)
        .merge(Toml::file(&path))
        // Capa 3: variables de entorno COMPLYX_*
        .merge(Env::prefixed("COMPLYX_").split("__"))
        .extract()
        .map_err(|e| anyhow::anyhow!("error cargando configuración desde '{}': {}", path, e))?;

    Ok(config)
}

impl AgentConfig {
    // Devuelve una instancia con todos los valores por defecto. Se usa como base en figment para
    // que los campos con `#[serde(default)]` funcionen correctamente al hacer merge.
    fn default_values() -> Self {
        Self {
            agent_id: String::new(),
            server_url: String::new(),
            enroll_url: String::new(),
            enroll_token: None,
            cert_dir: default_cert_dir(),
            db_path: default_db_path(),
            poll_interval_secs: default_poll_interval(),
            flush_interval_secs: default_flush_interval(),
            heartbeat_interval_secs: default_heartbeat_interval(),
            result_purge_days: default_purge_days(),
            auto_remediate: default_true(),
            log_level: default_log_level(),
            log_format: default_log_format(),
        }
    }

    // Valida que la configuración tiene los campos mínimos necesarios para arrancar el agente
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.server_url.is_empty() {
            anyhow::bail!(
                "server_url no configurado. \
                 Establécelo en {} o con COMPLYX_SERVER_URL",
                DEFAULT_CONFIG_PATH
            );
        }
        if self.enroll_url.is_empty() {
            anyhow::bail!(
                "enroll_url no configurado. \
                 Establécelo en {} o con COMPLYX_ENROLL_URL",
                DEFAULT_CONFIG_PATH
            );
        }
        Ok(())
    }

    // Devuelve `true` si el agente ya está registrado (tiene certificados en disco).
    pub fn is_enrolled(&self) -> bool {
        grpc_client::mtls::CertPaths::from_dir(&self.cert_dir).all_exist()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values_are_sensible() {
        let cfg = AgentConfig::default_values();
        assert_eq!(cfg.poll_interval_secs, 300);
        assert_eq!(cfg.flush_interval_secs, 60);
        assert!(cfg.auto_remediate);
        assert_eq!(cfg.log_level, "info");
    }

    #[test]
    fn validate_fails_without_server_url() {
        let cfg = AgentConfig::default_values();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_passes_with_required_fields() {
        let mut cfg = AgentConfig::default_values();
        cfg.server_url = "https://server:9000".into();
        cfg.enroll_url = "https://server:9001".into();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn is_enrolled_false_when_no_certs() {
        let mut cfg = AgentConfig::default_values();
        cfg.cert_dir = PathBuf::from("/tmp/complyx_test_no_certs_xyz");
        assert!(!cfg.is_enrolled());
    }
}
