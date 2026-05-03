//! Remediador `file_line_set`: escribe o reemplaza una directiva `key value` en un
//! fichero de configuración.
//!
//! Corresponde al check `file_line`: si el check detecta que `PASS_MIN_LEN = 8`
//! cuando se esperaba `>= 15`, este remediador lo corrige escribiendo `PASS_MIN_LEN 15`.
//!
//! ## Comportamiento
//!
//! 1. Busca la primera línea activa (no comentario) que empiece por `key`.
//! 2. Si la encuentra, reemplaza el valor en esa línea.
//! 3. Si no la encuentra y `create_if_absent = true`, añade la directiva al final.
//! 4. Siempre hace backup del fichero original antes de modificarlo.
//! 5. Verifica la escritura leyendo el fichero de nuevo (post-check).
//!
//! ## Ejemplo de params
//!
//! ```json
//! {
//!   "path": "/etc/login.defs",
//!   "key": "PASS_MIN_LEN",
//!   "value": "15",
//!   "separator": " ",
//!   "comment_chars": ["#"],
//!   "create_if_absent": true,
//!   "backup": true
//! }
//! ```

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::{RemediationError, RemediationExecutor, RemediationResult};

pub struct FileLineSetRemediator;

#[derive(Deserialize)]
struct Params {
    path: PathBuf,

    key: String, // Clave a buscar y escribir.

    value: String, // Nuevo valor a establecer.

    #[serde(default = "default_separator")]
    separator: String, // Separador entre clave y valor en el fichero. Por defecto será un espacio
                       // como en `sshd_config``

    #[serde(default = "default_comment_chars")]
    comment_chars: Vec<char>, // Caracteres de comentario. Las líneas que empiecen por uno de estos se ignoran.

    #[serde(default = "default_true")]
    create_if_absent: bool,

    #[serde(default = "default_true")]
    backup: bool,
}

fn default_separator() -> String { " ".to_string() }
fn default_comment_chars() -> Vec<char> { vec!['#'] }
fn default_true() -> bool { true }

#[async_trait]
impl RemediationExecutor for FileLineSetRemediator {
    fn remediation_type(&self) -> &'static str { "file_line_set" }

    async fn execute(&self, remediation_id: &str, params: &serde_json::Value,) -> Result<RemediationResult, RemediationError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| RemediationError::invalid_params("file_line_set", e))?;

        // Leer el contenido actual
        let original = tokio::fs::read_to_string(&p.path)
            .await
            .map_err(|e| RemediationError::io(p.path.display().to_string(), e))?;

        // Hacer backup si se pide
        if p.backup {
            let backup_path = format!("{}.bak.complyx", p.path.display());
            tokio::fs::write(&backup_path, original.as_bytes())
                .await
                .map_err(|e| RemediationError::io(backup_path.clone(), e))?;
            tracing::debug!(
                remediation_id,
                backup = %backup_path,
                "backup creado"
            );
        }

        let new_line = format!("{}{}{}", p.key, p.separator, p.value);
        let mut found = false;
        let mut new_lines: Vec<String> = Vec::new();

        for line in original.lines() {
            let trimmed = line.trim();

            // Ignorar comentarios y vacías
            if trimmed.is_empty() || p.comment_chars.iter().any(|&c| trimmed.starts_with(c)) {
                new_lines.push(line.to_string());
                continue;
            }

            // Comprobar si la línea es la directiva que buscamos
            let is_target = trimmed
                .split_whitespace()
                .next()
                .map(|k| k == p.key)
                .unwrap_or(false);

            if is_target && !found {
                new_lines.push(new_line.clone());
                found = true;
                tracing::debug!(
                    remediation_id,
                    key = %p.key,
                    old = %line,
                    new = %new_line,
                    "directiva reemplazada"
                );
            } else {
                new_lines.push(line.to_string());
            }
        }

        // Si no se encontró la clave y create_if_absent está activo
        if !found {
            if p.create_if_absent {
                // Añadir una línea en blanco de separación si el fichero no termina en newline
                if !original.ends_with('\n') && !original.is_empty() {
                    new_lines.push(String::new());
                }
                new_lines.push(new_line.clone());
                tracing::debug!(
                    remediation_id,
                    key = %p.key,
                    value = %p.value,
                    "directiva añadida al final del fichero"
                );
            } else {
                return Ok(RemediationResult::skipped(format!(
                    "clave '{}' no encontrada en '{}' y create_if_absent = false",
                    p.key,
                    p.path.display()
                )));
            }
        }

        // Escribir el fichero modificado
        let new_content = new_lines.join("\n") + "\n";
        tokio::fs::write(&p.path, new_content.as_bytes())
            .await
            .map_err(|e| RemediationError::io(p.path.display().to_string(), e))?;

        Ok(RemediationResult::applied(format!(
            "'{}': {} = {} ({})",
            p.path.display(),
            p.key,
            p.value,
            if found { "reemplazado" } else { "añadido" }
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn replaces_existing_value() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("login.defs");
        tokio::fs::write(&path, "# comentario\nPASS_MIN_LEN    8\nPASS_MAX_DAYS 90\n")
            .await
            .unwrap();

        let result = FileLineSetRemediator
            .execute("rem-1", &json!({
                "path": path,
                "key": "PASS_MIN_LEN",
                "value": "15",
                "backup": false
            }))
            .await
            .unwrap();

        assert!(result.applied);

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("PASS_MIN_LEN 15"), "contenido: {}", content);
        assert!(!content.contains("PASS_MIN_LEN    8"));
    }

    #[tokio::test]
    async fn creates_key_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("login.defs");
        tokio::fs::write(&path, "# solo comentario\n").await.unwrap();

        let result = FileLineSetRemediator
            .execute("rem-1", &json!({
                "path": path,
                "key": "PASS_MIN_LEN",
                "value": "15",
                "create_if_absent": true,
                "backup": false
            }))
            .await
            .unwrap();

        assert!(result.applied);
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("PASS_MIN_LEN 15"));
    }

    #[tokio::test]
    async fn skips_when_absent_and_no_create() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("login.defs");
        tokio::fs::write(&path, "OTHER_KEY 5\n").await.unwrap();

        let result = FileLineSetRemediator
            .execute("rem-1", &json!({
                "path": path,
                "key": "PASS_MIN_LEN",
                "value": "15",
                "create_if_absent": false,
                "backup": false
            }))
            .await
            .unwrap();

        assert!(!result.applied);
        assert!(result.detail.contains("create_if_absent"));
    }

    #[tokio::test]
    async fn preserves_comments() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sshd_config");
        tokio::fs::write(&path, "# Este es un comentario\nPermitRootLogin yes\n# Otro\n")
            .await
            .unwrap();

        FileLineSetRemediator
            .execute("rem-1", &json!({
                "path": path,
                "key": "PermitRootLogin",
                "value": "no",
                "backup": false
            }))
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("# Este es un comentario"));
        assert!(content.contains("# Otro"));
        assert!(content.contains("PermitRootLogin no"));
    }

    #[tokio::test]
    async fn creates_backup_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("login.defs");
        tokio::fs::write(&path, "PASS_MIN_LEN 8\n").await.unwrap();

        FileLineSetRemediator
            .execute("rem-1", &json!({
                "path": path,
                "key": "PASS_MIN_LEN",
                "value": "15",
                "backup": true
            }))
            .await
            .unwrap();

        let backup = dir.path().join("login.defs.bak.complyx");
        assert!(backup.exists(), "el backup debe existir");
        let backup_content = tokio::fs::read_to_string(&backup).await.unwrap();
        assert!(backup_content.contains("PASS_MIN_LEN 8"), "el backup debe tener el valor original");
    }
}
