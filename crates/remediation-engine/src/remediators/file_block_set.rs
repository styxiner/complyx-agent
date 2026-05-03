//! Remediador `file_block_set`: asegura que un bloque de texto está presente en un fichero.
//!
//! Corresponde al check `file_block`. Si el check detecta que un patrón requerido
//! no está presente, este remediador añade el bloque de texto configurado.
//!
//! ## Comportamiento
//!
//! - Si el bloque ya está presente (idempotencia), no hace nada.
//! - Si no está, lo añade al final del fichero (o en la posición indicada).
//! - Siempre hace backup antes de modificar.
//!
//! ## Ejemplo de params
//!
//! ```json
//! {
//!   "path": "/etc/pam.d/common-password",
//!   "block": "password requisite pam_pwquality.so retry=3 minlen=15",
//!   "marker": "pam_pwquality",
//!   "backup": true
//! }
//! ```

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::{RemediationError, RemediationExecutor, RemediationResult};

pub struct FileBlockSetRemediator;

#[derive(Deserialize)]
struct Params {
    path: PathBuf,

    block: String,

    #[serde(default)]
    marker: Option<String>, // Subcadena que se usa para comprobar si el bloque ya esta presente.
                            // Si se omite, se usa `block` completo como marker. Util cuando el
                            // bloque tiene partes variables pero el marker es fijo.

    #[serde(default = "default_true")]
    backup: bool,

    /// Línea antes de la cual insertar el bloque. Si es `None`, se añade al final.
    /// Útil para insertar antes de una línea concreta (ej. antes de `# END`).
    #[serde(default)]
    insert_before: Option<String>, // Si es `None`, se añade al final. Util para inser
}

fn default_true() -> bool { true }

#[async_trait]
impl RemediationExecutor for FileBlockSetRemediator {
    fn remediation_type(&self) -> &'static str { "file_block_set" }

    async fn execute(
        &self,
        remediation_id: &str,
        params: &serde_json::Value,
    ) -> Result<RemediationResult, RemediationError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| RemediationError::invalid_params("file_block_set", e))?;

        let original = tokio::fs::read_to_string(&p.path)
            .await
            .map_err(|e| RemediationError::io(p.path.display().to_string(), e))?;

        // Comprobar si el bloque ya está presente (idempotencia)
        let check_marker = p.marker.as_deref().unwrap_or(&p.block);
        if original.contains(check_marker) {
            return Ok(RemediationResult::skipped(format!(
                "'{}': bloque ya presente (marker: '{}')",
                p.path.display(),
                check_marker
            )));
        }

        // Backup
        if p.backup {
            let backup_path = format!("{}.bak.complyx", p.path.display());
            tokio::fs::write(&backup_path, original.as_bytes())
                .await
                .map_err(|e| RemediationError::io(backup_path.clone(), e))?;
        }

        // Construir el nuevo contenido
        let new_content = if let Some(before_marker) = &p.insert_before {
            // Insertar antes de la línea que contenga before_marker
            let mut lines = Vec::new();
            let mut inserted = false;
            for line in original.lines() {
                if !inserted && line.contains(before_marker.as_str()) {
                    lines.push(p.block.clone());
                    inserted = true;
                }
                lines.push(line.to_string());
            }
            if !inserted {
                // Si no se encontró el marcador, añadir al final
                tracing::warn!(
                    remediation_id,
                    insert_before = %before_marker,
                    "marcador insert_before no encontrado, añadiendo al final"
                );
                lines.push(p.block.clone());
            }
            lines.join("\n") + "\n"
        } else {
            // Añadir al final
            let separator = if original.ends_with('\n') || original.is_empty() {
                ""
            } else {
                "\n"
            };
            format!("{}{}{}\n", original, separator, p.block)
        };

        tokio::fs::write(&p.path, new_content.as_bytes())
            .await
            .map_err(|e| RemediationError::io(p.path.display().to_string(), e))?;

        tracing::info!(
            remediation_id,
            path = %p.path.display(),
            "bloque añadido al fichero"
        );

        Ok(RemediationResult::applied(format!(
            "'{}': bloque añadido",
            p.path.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn adds_block_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("common-password");
        tokio::fs::write(&path, "password [success=1] pam_unix.so\n")
            .await
            .unwrap();

        let result = FileBlockSetRemediator
            .execute("rem-1", &json!({
                "path": path,
                "block": "password requisite pam_pwquality.so minlen=15",
                "backup": false
            }))
            .await
            .unwrap();

        assert!(result.applied);
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("pam_pwquality.so minlen=15"));
    }

    #[tokio::test]
    async fn skips_when_block_already_present() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("common-password");
        tokio::fs::write(&path, "password requisite pam_pwquality.so minlen=15\n")
            .await
            .unwrap();

        let result = FileBlockSetRemediator
            .execute("rem-1", &json!({
                "path": path,
                "block": "password requisite pam_pwquality.so minlen=15",
                "backup": false
            }))
            .await
            .unwrap();

        // Idempotente: no modifica si ya está
        assert!(!result.applied);
        assert!(result.detail.contains("ya presente"));
    }

    #[tokio::test]
    async fn inserts_before_marker() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config");
        tokio::fs::write(&path, "line1\n# END MANAGED\nline3\n")
            .await
            .unwrap();

        FileBlockSetRemediator
            .execute("rem-1", &json!({
                "path": path,
                "block": "new_directive = value",
                "insert_before": "# END MANAGED",
                "backup": false
            }))
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let pos_new = content.find("new_directive").unwrap();
        let pos_end = content.find("# END MANAGED").unwrap();
        assert!(pos_new < pos_end, "el bloque debe ir antes del marcador");
    }

    #[tokio::test]
    async fn marker_param_used_for_presence_check() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config");
        // El fichero tiene pam_pwquality pero con distintos params
        tokio::fs::write(&path, "password requisite pam_pwquality.so minlen=8\n")
            .await
            .unwrap();

        let result = FileBlockSetRemediator
            .execute("rem-1", &json!({
                "path": path,
                "block": "password requisite pam_pwquality.so minlen=15",
                "marker": "pam_pwquality",  // ya existe → skip
                "backup": false
            }))
            .await
            .unwrap();

        assert!(!result.applied, "debe saltar porque el marker ya está presente");
    }
}
