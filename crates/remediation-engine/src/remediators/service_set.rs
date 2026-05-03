//! Remediador `service_set`: activa/desactiva o arranca/para un servicio systemd.
//!
//! Invoca `systemctl` con los subcomandos necesarios según los campos `active`
//! y `enabled` que se especifiquen en los params.
//!
//! ## Ejemplo de params
//!
//! ```json
//! {
//!   "name": "auditd",
//!   "active": true,
//!   "enabled": true
//! }
//! ```

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::{RemediationError, RemediationExecutor, RemediationResult};

pub struct ServiceSetRemediator;

#[derive(Deserialize)]
struct Params {
    name: String, // Sin sufijo ".service", se añade si hace falta

    #[serde(default)]
    active: Option<bool>,

    #[serde(default)]
    enabled: Option<bool>,
}

#[async_trait]
impl RemediationExecutor for ServiceSetRemediator {
    fn remediation_type(&self) -> &'static str { "service_set" }

    async fn execute(&self, remediation_id: &str, params: &serde_json::Value,) -> Result<RemediationResult, RemediationError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| RemediationError::invalid_params("service_set", e))?;

        if p.active.is_none() && p.enabled.is_none() {
            return Err(RemediationError::Internal {
                remediation_type: "service_set".into(),
                reason: "debe especificar al menos 'active' o 'enabled'".into(),
            });
        }

        let service = normalize_service_name(&p.name);
        let mut actions_applied: Vec<String> = Vec::new();

        // Gestionar enabled/disabled primero (es más seguro hacerlo antes de start/stop)
        if let Some(should_enable) = p.enabled {
            let subcommand = if should_enable { "enable" } else { "disable" };
            run_systemctl(remediation_id, subcommand, &service).await?;
            actions_applied.push(if should_enable {
                "habilitado".to_string()
            } else {
                "deshabilitado".to_string()
            });
        }

        // Gestionar active/inactive
        if let Some(should_be_active) = p.active {
            let subcommand = if should_be_active { 
                "start"
            } else { 
                "stop" 
            };

            run_systemctl(remediation_id, subcommand, &service).await?;
            actions_applied.push(if should_be_active {
                "arrancado".to_string()
            } else {
                "parado".to_string()
            });
        }

        Ok(RemediationResult::applied(format!(
            "servicio '{}': {}",
            service,
            actions_applied.join(", ")
        )))
    }
}

fn normalize_service_name(name: &str) -> String {
    if name.contains('.') { name.to_string() } else { format!("{}.service", name) }
}

async fn run_systemctl(remediation_id: &str, subcommand: &str, service: &str,)-> Result<(), RemediationError> {
    tracing::info!(remediation_id, subcommand, service, "ejecutando systemctl");

    let output = tokio::process::Command::new("systemctl")
        .args([subcommand, service])
        .output()
        .await
        .map_err(|e| RemediationError::Internal {
            remediation_type: "service_set".into(),
            reason: format!("no se pudo ejecutar systemctl: {}", e),
        })?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(RemediationError::Internal {
            remediation_type: "service_set".into(),
            reason: format!(
                "systemctl {} {} falló (exit {}): {}",
                subcommand,
                service,
                output.status.code().unwrap_or(-1),
                stderr,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_adds_service_suffix() {
        assert_eq!(normalize_service_name("sshd"), "sshd.service");
        assert_eq!(normalize_service_name("sshd.service"), "sshd.service");
        assert_eq!(normalize_service_name("dbus.socket"), "dbus.socket");
    }
}
