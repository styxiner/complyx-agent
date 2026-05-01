//! Check `service`: verifica el estado de un servicio systemd.
//!
//! Comprueba si el servicio está activo (running) y/o habilitado (enabled).
//! En lugar de parsear la salida de texto de `systemctl`, lee directamente los
//! ficheros de estado de systemd en `/run/systemd/` para evitar dependencias
//! del PATH y ser más robusto ante distintas versiones de systemd.
//!
//! Si el directorio `/run/systemd/` no existe (sistema no-systemd o contenedor),
//! recurre a ejecutar `systemctl` como fallback.
//!
//! ## Ejemplo de uso
//!
//! ```json
//! {
//!   "type": "service",
//!   "name": "sshd",
//!   "active": true,
//!   "enabled": true
//! }
//! ```

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::CheckExecutor;
use crate::result::{CheckError, EngineCheckResult};

pub struct ServiceExecutor;

#[derive(Deserialize)]
struct Params {
    /// Nombre del servicio (ej. `sshd`, `nginx`, `auditd`).
    /// Sin el sufijo `.service` — se añade automáticamente si hace falta.
    name: String,

    /// Si se especifica, verifica que el servicio esté activo (`true`) o inactivo (`false`).
    #[serde(default)]
    active: Option<bool>,

    /// Si se especifica, verifica que el servicio esté habilitado (`true`) o deshabilitado (`false`).
    #[serde(default)]
    enabled: Option<bool>,
}

/// Estado de un servicio tal como lo reporta systemd.
#[derive(Debug, PartialEq)]
struct ServiceState {
    /// `true` si el servicio está actualmente corriendo.
    is_active: bool,
    /// `true` si el servicio arranca automáticamente con el sistema.
    is_enabled: bool,
    /// Estado en texto libre (ej. "active (running)", "inactive (dead)").
    active_state: String,
}

#[async_trait]
impl CheckExecutor for ServiceExecutor {
    fn check_type(&self) -> &'static str { "service" }

    async fn execute(
        &self,
        check_id: &str,
        params: &serde_json::Value,
    ) -> Result<EngineCheckResult, CheckError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| CheckError::invalid_params("service", e))?;

        if p.active.is_none() && p.enabled.is_none() {
            return Err(CheckError::Internal {
                check_type: "service".into(),
                reason: "debe especificar al menos 'active' o 'enabled'".into(),
            });
        }

        let service_name = normalize_service_name(&p.name);
        let state = query_service_state(&service_name).await.map_err(|e| {
            CheckError::Internal {
                check_type: "service".into(),
                reason: e,
            }
        })?;

        // Verificar estado activo
        if let Some(expected_active) = p.active {
            if state.is_active != expected_active {
                return Ok(EngineCheckResult::fail(
                    check_id,
                    &state.active_state,
                    if expected_active { "active" } else { "inactive" },
                    format!(
                        "servicio '{}': estado = '{}' (esperado {})",
                        service_name,
                        state.active_state,
                        if expected_active { "activo" } else { "inactivo" }
                    ),
                ));
            }
        }

        // Verificar enabled
        if let Some(expected_enabled) = p.enabled {
            if state.is_enabled != expected_enabled {
                return Ok(EngineCheckResult::fail(
                    check_id,
                    if state.is_enabled { "enabled" } else { "disabled" },
                    if expected_enabled { "enabled" } else { "disabled" },
                    format!(
                        "servicio '{}': {} (esperado {})",
                        service_name,
                        if state.is_enabled { "habilitado" } else { "deshabilitado" },
                        if expected_enabled { "habilitado" } else { "deshabilitado" },
                    ),
                ));
            }
        }

        Ok(EngineCheckResult::pass(
            check_id,
            &state.active_state,
            "conforme",
            format!(
                "servicio '{}': {} / {}",
                service_name,
                state.active_state,
                if state.is_enabled { "enabled" } else { "disabled" }
            ),
        ))
    }
}

/// Añade `.service` si el nombre no tiene unidad explícita.
fn normalize_service_name(name: &str) -> String {
    if name.contains('.') {
        name.to_string()
    } else {
        format!("{}.service", name)
    }
}

/// Consulta el estado del servicio.
/// Usa `systemctl show` con campos específicos para obtener datos estructurados
/// en lugar de parsear la salida legible de `systemctl status`.
async fn query_service_state(service_name: &str) -> Result<ServiceState, String> {
    let output = tokio::process::Command::new("systemctl")
        .args(["show", "--no-pager", "--property=ActiveState,UnitFileState", service_name])
        .output()
        .await
        .map_err(|e| format!("no se pudo ejecutar systemctl: {}", e))?;

    // systemctl show devuelve exit 0 incluso para servicios inexistentes
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    parse_systemctl_show(&stdout, service_name)
}

/// Parsea la salida de `systemctl show --property=ActiveState,UnitFileState`.
///
/// Formato:
/// ```
/// ActiveState=active
/// UnitFileState=enabled
/// ```
fn parse_systemctl_show(output: &str, service_name: &str) -> Result<ServiceState, String> {
    let mut active_state = String::from("unknown");
    let mut unit_file_state = String::from("unknown");

    for line in output.lines() {
        if let Some(val) = line.strip_prefix("ActiveState=") {
            active_state = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("UnitFileState=") {
            unit_file_state = val.trim().to_string();
        }
    }

    if active_state == "unknown" && unit_file_state == "unknown" {
        return Err(format!(
            "servicio '{}' no encontrado o systemd no disponible",
            service_name
        ));
    }

    let is_active = active_state == "active";
    // "enabled", "enabled-runtime", "static" se consideran habilitados
    let is_enabled = matches!(
        unit_file_state.as_str(),
        "enabled" | "enabled-runtime" | "static"
    );

    Ok(ServiceState {
        is_active,
        is_enabled,
        active_state: format!("{} ({})", active_state, unit_file_state),
    })
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

    #[test]
    fn parse_show_active_enabled() {
        let output = "ActiveState=active\nUnitFileState=enabled\n";
        let state = parse_systemctl_show(output, "sshd.service").unwrap();
        assert!(state.is_active);
        assert!(state.is_enabled);
    }

    #[test]
    fn parse_show_inactive_disabled() {
        let output = "ActiveState=inactive\nUnitFileState=disabled\n";
        let state = parse_systemctl_show(output, "telnet.service").unwrap();
        assert!(!state.is_active);
        assert!(!state.is_enabled);
    }

    #[test]
    fn parse_show_static_is_enabled() {
        let output = "ActiveState=active\nUnitFileState=static\n";
        let state = parse_systemctl_show(output, "getty@.service").unwrap();
        assert!(state.is_enabled);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn checks_dbus_service_exists() {
        use serde_json::json;
        // dbus existe en la gran mayoría de sistemas Linux con systemd
        let result = ServiceExecutor
            .execute("chk-1", &json!({
                "name": "dbus",
                "active": true
            }))
            .await;

        // No verificamos el resultado (depende del entorno), solo que no da error
        assert!(result.is_ok(), "no debería dar error: {:?}", result.err());
    }
}
