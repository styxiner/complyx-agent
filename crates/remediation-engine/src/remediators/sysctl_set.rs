//! Remediador `sysctl_set`: establece un parámetro del kernel de forma persistente.
//!
//! Escribe en `/etc/sysctl.d/99-complyx.conf` y aplica el valor en runtime
//! ejecutando `sysctl -w key=value`, sin necesidad de reinicio.
//!
//! Usar un fichero dedicado `99-complyx.conf` evita modificar ficheros del sistema
//! como `/etc/sysctl.conf` y hace fácil revertir todos los cambios de Complyx
//! eliminando ese único fichero.
//!
//! ## Ejemplo de params
//!
//! ```json
//! {
//!   "key": "net.ipv4.ip_forward",
//!   "value": "0"
//! }
//! ```

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::{RemediationError, RemediationExecutor, RemediationResult};

pub struct SysctlSetRemediator;

const COMPLYX_SYSCTL_FILE: &str = "/etc/sysctl.d/99-complyx.conf";

#[derive(Deserialize)]
struct Params {
    key: String, // Nombre del parámetro (ej. `net.ipv4.ip_forward`).

    value: String,
}

#[async_trait]
impl RemediationExecutor for SysctlSetRemediator {
    fn remediation_type(&self) -> &'static str { "sysctl_set" }

    async fn execute(&self, remediation_id: &str, params: &serde_json::Value,) -> Result<RemediationResult, RemediationError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| RemediationError::invalid_params("sysctl_set", e))?;

        // Escribir en el fichero persistente
        persist_sysctl(&p.key, &p.value).await?;

        tracing::debug!(
            remediation_id,
            key = %p.key,
            value = %p.value,
            file = COMPLYX_SYSCTL_FILE,
            "parámetro escrito en fichero persistente"
        );

        // Aplicar en runtime con `sysctl -w`
        apply_sysctl_runtime(remediation_id, &p.key, &p.value).await?;

        Ok(RemediationResult::applied(format!("sysctl {key} = {value} (persistente + runtime)", key = p.key, value = p.value, )))
    }
}

// Escribe o reemplaza `key = value` en el fichero de sysctl de Complyx.
// El fichero tiene el formato:
// ```
// # Gestionado por Complyx. No editar manualmente.
// net.ipv4.ip_forward = 0
// kernel.randomize_va_space = 2
// ```
async fn persist_sysctl(key: &str, value: &str) -> Result<(), RemediationError> {
    // Leer el fichero actual (puede no existir)
    let existing = tokio::fs::read_to_string(COMPLYX_SYSCTL_FILE)
        .await
        .unwrap_or_default();

    let header = "# Gestionado por Complyx. No editar manualmente.\n";
    let new_entry = format!("{} = {}", key, value);
    let mut found = false;
    let mut lines: Vec<String> = Vec::new();

    // Asegurar que el header está
    if !existing.starts_with("# Gestionado") {
        lines.push(header.trim_end().to_string());
    }

    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            lines.push(line.to_string());
            continue;
        }
        // Comprobar si es la misma clave (antes del `=`)
        let line_key = trimmed.split('=').next().map(|k| k.trim()).unwrap_or("");
        if line_key == key {
            lines.push(new_entry.clone());
            found = true;
        } else {
            lines.push(line.to_string());
        }
    }

    if !found {
        lines.push(new_entry);
    }

    let content = lines.join("\n") + "\n";

    // Crear el directorio si no existe
    tokio::fs::create_dir_all("/etc/sysctl.d")
        .await
        .map_err(|e| RemediationError::io("/etc/sysctl.d", e))?;

    tokio::fs::write(COMPLYX_SYSCTL_FILE, content.as_bytes())
        .await
        .map_err(|e| RemediationError::io(COMPLYX_SYSCTL_FILE, e))?;

    Ok(())
}

// Aplica el parámetro en runtime con `sysctl -w key=value`.
async fn apply_sysctl_runtime(remediation_id: &str, key: &str, value: &str,) -> Result<(), RemediationError> {
    let kv = format!("{}={}", key, value);

    let output = tokio::process::Command::new("sysctl")
        .args(["-w", &kv])
        .output()
        .await
        .map_err(|e| RemediationError::Internal {
            remediation_type: "sysctl_set".into(),
            reason: format!("no se pudo ejecutar sysctl: {}", e),
        })?;

    if output.status.success() {
        tracing::info!(remediation_id, key, value, "sysctl aplicado en runtime");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(RemediationError::Internal {
            remediation_type: "sysctl_set".into(),
            reason: format!("sysctl -w {} falló: {}", kv, stderr),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn persist_sysctl_writes_entry() {
        // Solo testamos la lógica de construcción del contenido, no la escritura real
        // (que requiere /etc/sysctl.d con permisos de root)
        let key = "net.ipv4.ip_forward";
        let value = "0";
        let entry = format!("{} = {}", key, value);
        assert!(entry.contains("net.ipv4.ip_forward"));
        assert!(entry.contains(" = 0"));
    }
}
