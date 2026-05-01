//! Check `file_line`: busca una línea con formato `key <sep> value` y compara el valor
//! con un operador.
//!
//! Diseñado para ficheros de configuración tipo `sshd_config`, `login.defs` y `sysctl.conf`
//! donde cada directiva ocupa una línea con separador configurable.
//!
//! ## Ejemplo de uso
//!
//! ```json
//! {
//!   "type": "file_line",
//!   "path": "/etc/login.defs",
//!   "key": "PASS_MIN_LEN",
//!   "operator": ">=",
//!   "value": "15",
//!   "separator": "\\s+",
//!   "comment_chars": ["#"],
//!   "required": true
//! }
//! ```

use std::path::PathBuf;

use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;

use crate::executor::{CheckExecutor, CompareOperator};
use crate::result::{CheckError, EngineCheckResult};

pub struct FileLineExecutor;

#[derive(Deserialize)]
struct Params {
    path: PathBuf,

    key: String,

    operator: CompareOperator,

    value: String,

    #[serde(default = "default_separator")]
    separator: String, // Regex que actua como separador entre clave y valor. Defecto: `\S+` (un
                       // espacio/tab o más. Es el formato de sshd_config por ejemplo)
                       // Usar `=\\s*` para ficheros tipo clave=valor o clave = valor

    #[serde(default = "default_comment_chars")]
    comment_chars: Vec<char>, // Caracteres que indican inicio de comentario. Las lineas que
                              // use por cualquiera de estos caracteres (tras trim) se
                              // ignoran.

    #[serde(default = "default_true")]
    required: bool, // Si `true` (por defecto), el check falla si la clave no existe en el fichero.
                    // Si `false`, una clave ausente se considera conforme (que la directiva no
                    // aplica vaya)
}

fn default_separator() -> String { 
    r"\s+".to_string() 
}

fn default_comment_chars() -> Vec<char> { 
    vec!['#'] 
}

fn default_true() -> bool { 
    true 
}

#[async_trait]
impl CheckExecutor for FileLineExecutor {
    fn check_type(&self) -> &'static str { 
        "file_line" 
    }

    async fn execute(&self, check_id: &str, params: &serde_json::Value,) -> Result<EngineCheckResult, CheckError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| CheckError::invalid_params("file_line", e))?;

        let content = tokio::fs::read_to_string(&p.path)
            .await
            .map_err(|e| CheckError::io(p.path.display().to_string(), e))?;

        let sep_re = Regex::new(&p.separator).map_err(|e| CheckError::Internal {
            check_type: "file_line".into(),
            reason: format!("separador regex inválido '{}': {}", p.separator, e),
        })?;

        for line in content.lines() {
            let trimmed = line.trim();

            // Ignorar líneas vacías y comentarios
            if trimmed.is_empty() || p.comment_chars.iter().any(|&c| trimmed.starts_with(c)) {
                continue;
            }

            // Dividir en como máximo 2 partes: clave y resto
            let parts: Vec<&str> = sep_re.splitn(trimmed, 2).collect();
            if parts.len() < 2 {
                continue;
            }

            let found_key = parts[0].trim();
            if found_key != p.key {
                continue;
            }

            // Clave encontrada: comparar el valor
            let actual = parts[1].trim();
            let passed = p.operator.compare(actual, &p.value);
            let expected_str = format!("{} {}", p.operator, p.value);
            let detail = EngineCheckResult::value_detail(&p.key, actual, &p.operator, &p.value, passed,);

            return Ok(if passed {
                EngineCheckResult::pass(check_id, actual, &expected_str, detail)
            } else {
                EngineCheckResult::fail(check_id, actual, &expected_str, detail)
            });
        }

        // Clave no encontrada en el fichero
        if p.required {
            Ok(EngineCheckResult::fail(
                check_id,
                "ausente",
                &format!("{} {}", p.operator, p.value),
                format!("clave '{}' no encontrada en '{}'", p.key, p.path.display()),
            ))
        } else {
            Ok(EngineCheckResult::pass(
                check_id,
                "ausente",
                "no requerida",
                format!("clave '{}' no encontrada en '{}' (no requerida)", p.key, p.path.display()),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    async fn write_file(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        tokio::fs::write(&path, content).await.unwrap();
        path
    }

    #[tokio::test]
    async fn passes_when_value_satisfies_operator() {
        let dir = tempdir().unwrap();
        let path = write_file(
            dir.path(), "login.defs",
            "# comentario\nPASS_MIN_LEN    15\nPASS_MAX_DAYS   90\n",
        ).await;

        let result = FileLineExecutor.execute("chk-1", &json!({
            "path": path,
            "key": "PASS_MIN_LEN",
            "operator": ">=",
            "value": "15"
        })).await.unwrap();

        assert!(result.passed);
        assert_eq!(result.actual_value, "15");
    }

    #[tokio::test]
    async fn fails_when_value_too_low() {
        let dir = tempdir().unwrap();
        let path = write_file(dir.path(), "login.defs", "PASS_MIN_LEN    8\n").await;

        let result = FileLineExecutor.execute("chk-1", &json!({
            "path": path,
            "key": "PASS_MIN_LEN",
            "operator": ">=",
            "value": "15"
        })).await.unwrap();

        assert!(!result.passed);
        assert_eq!(result.actual_value, "8");
        assert!(result.detail.contains("esperado"));
    }

    #[tokio::test]
    async fn fails_when_key_absent_and_required() {
        let dir = tempdir().unwrap();
        let path = write_file(dir.path(), "login.defs", "# vacío\n").await;

        let result = FileLineExecutor.execute("chk-1", &json!({
            "path": path,
            "key": "PASS_MIN_LEN",
            "operator": ">=",
            "value": "15",
            "required": true
        })).await.unwrap();

        assert!(!result.passed);
        assert!(result.detail.contains("no encontrada"));
    }

    #[tokio::test]
    async fn passes_when_key_absent_and_not_required() {
        let dir = tempdir().unwrap();
        let path = write_file(dir.path(), "login.defs", "# vacío\n").await;

        let result = FileLineExecutor.execute("chk-1", &json!({
            "path": path,
            "key": "PASS_MIN_LEN",
            "operator": ">=",
            "value": "15",
            "required": false
        })).await.unwrap();

        assert!(result.passed);
    }

    #[tokio::test]
    async fn ignores_comment_lines() {
        let dir = tempdir().unwrap();
        // La línea comentada tiene el valor incorrecto; la activa tiene el correcto
        let path = write_file(
            dir.path(), "sshd_config",
            "#PASS_MIN_LEN 5\nPASS_MIN_LEN 20\n",
        ).await;

        let result = FileLineExecutor.execute("chk-1", &json!({
            "path": path,
            "key": "PASS_MIN_LEN",
            "operator": ">=",
            "value": "15"
        })).await.unwrap();

        assert!(result.passed);
        assert_eq!(result.actual_value, "20");
    }

    #[tokio::test]
    async fn works_with_equals_separator_for_sysctl() {
        let dir = tempdir().unwrap();
        let path = write_file(
            dir.path(), "sysctl.conf",
            "net.ipv4.ip_forward = 0\n",
        ).await;

        let result = FileLineExecutor.execute("chk-1", &json!({
            "path": path,
            "key": "net.ipv4.ip_forward",
            "operator": "=",
            "value": "0",
            "separator": r"\s*=\s*"
        })).await.unwrap();

        assert!(result.passed);
    }

    #[tokio::test]
    async fn works_with_contains_operator() {
        let dir = tempdir().unwrap();
        let path = write_file(
            dir.path(), "sshd_config",
            "AllowedCiphers aes256-ctr,aes128-ctr\n",
        ).await;

        let result = FileLineExecutor.execute("chk-1", &json!({
            "path": path,
            "key": "AllowedCiphers",
            "operator": "contains",
            "value": "aes256-ctr"
        })).await.unwrap();

        assert!(result.passed);
    }
}
