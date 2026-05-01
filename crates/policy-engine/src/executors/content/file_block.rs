//! Check `file_block`: verifica que un fichero contiene (o no contiene) patrones.
//!
//! Pensado para ficheros donde la estructura no es `key = value` línea a línea, sino
//! bloques de texto más complejos: `pam.d`, `sudoers`, secciones de `sshd_config`, etc.
//!
//! Todos los patrones de `must_contain` deben estar presentes, y ninguno de
//! `must_not_contain` debe aparecer en el fichero.
//!
//! ## Ejemplo de uso
//!
//! ```json
//! {
//!   "type": "file_block",
//!   "path": "/etc/pam.d/common-password",
//!   "must_contain": [
//!     "password.*requisite.*pam_pwquality",
//!     "minlen=15"
//!   ],
//!   "must_not_contain": ["nullok"],
//!   "match_mode": "regex"
//! }
//! ```

use std::path::PathBuf;

use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;

use crate::executor::CheckExecutor;
use crate::result::{CheckError, EngineCheckResult};

pub struct FileBlockExecutor;

// Modo de coincidencia de los patrones.
#[derive(Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum MatchMode {
    #[default]
    Regex,

    Literal,
}

#[derive(Deserialize)]
struct Params {
    path: PathBuf,

    #[serde(default)]
    must_contain: Vec<String>,

    #[serde(default)]
    must_not_contain: Vec<String>,

    #[serde(default)]
    match_mode: MatchMode, // Si va a ser regex o literal

    /// Si `true`, la comparación es insensible a mayúsculas/minúsculas.
    /// Solo aplica en modo `literal`. En modo `regex`, usar el flag `(?i)` en el patrón.
    #[serde(default)]
    case_insensitive: bool, // Solo aplica en modo literal. En modo regex, usar el flag `(?i)` en
                            // el patron
}

#[async_trait]
impl CheckExecutor for FileBlockExecutor {
    fn check_type(&self) -> &'static str { "file_block" }

    async fn execute(&self, check_id: &str, params: &serde_json::Value,) -> Result<EngineCheckResult, CheckError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| CheckError::invalid_params("file_block", e))?;

        if p.must_contain.is_empty() && p.must_not_contain.is_empty() {
            return Err(CheckError::Internal {
                check_type: "file_block".into(),
                reason: "debe especificar al menos un patrón en must_contain o must_not_contain"
                    .into(),
            });
        }

        let content = tokio::fs::read_to_string(&p.path)
            .await
            .map_err(|e| CheckError::io(p.path.display().to_string(), e))?;

        let content_for_match = if p.case_insensitive {
            content.to_lowercase()
        } else {
            content.clone()
        };

        // Verificar must_contain
        for pattern in &p.must_contain {
            let found = match_pattern(pattern, &content_for_match, p.case_insensitive, &p.match_mode)
                .map_err(|e| CheckError::Internal {
                    check_type: "file_block".into(),
                    reason: format!("patrón regex inválido '{}': {}", pattern, e),
                })?;

            if !found {
                return Ok(EngineCheckResult::fail(
                    check_id,
                    "ausente",
                    &format!("contiene '{}'", pattern),
                    format!("'{}': patrón requerido no encontrado: '{}'", p.path.display(),
                        pattern
                    ),
                ));
            }
        }

        // Verificar must_not_contain
        for pattern in &p.must_not_contain {
            let found = match_pattern(pattern, &content_for_match, p.case_insensitive, &p.match_mode)
                .map_err(|e| CheckError::Internal {
                    check_type: "file_block".into(),
                    reason: format!("patrón regex inválido '{}': {}", pattern, e),
                })?;

            if found {
                return Ok(EngineCheckResult::fail(
                    check_id,
                    &format!("contiene '{}'", pattern),
                    "ausente",
                    format!(
                        "'{}': patrón prohibido encontrado: '{}'",
                        p.path.display(),
                        pattern
                    ),
                ));
            }
        }

        let summary = build_summary(&p.must_contain, &p.must_not_contain);
        Ok(EngineCheckResult::pass(
            check_id,
            "conforme",
            "conforme",
            format!("'{}': {}", p.path.display(), summary),
        ))
    }
}

// Comprueba si `pattern` aparece en `content` según el modo de matching.
fn match_pattern(pattern: &str, content: &str, case_insensitive: bool, mode: &MatchMode,) -> Result<bool, String> {
    match mode {
        MatchMode::Literal => {
            let pat = if case_insensitive {
                pattern.to_lowercase()
            } else {
                pattern.to_string()
            };
            Ok(content.contains(pat.as_str()))
        }
        MatchMode::Regex => {
            // En modo regex se busca en el contenido original (el flag (?i) maneja el caso)
            let re = Regex::new(pattern).map_err(|e| e.to_string())?;
            Ok(re.is_match(content))
        }
    }
}

fn build_summary(must_contain: &[String], must_not_contain: &[String]) -> String {
    let mut parts = Vec::new();
    if !must_contain.is_empty() {
        parts.push(format!("{} patrón(es) requerido(s) presentes", must_contain.len()));
    }
    if !must_not_contain.is_empty() {
        parts.push(format!("{} patrón(es) prohibido(s) ausentes", must_not_contain.len()));
    }
    parts.join(", ")
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
    async fn passes_when_all_required_patterns_present() {
        let dir = tempdir().unwrap();
        let path = write_file(
            dir.path(), "common-password",
            "password requisite pam_pwquality.so retry=3 minlen=15\n\
             password [success=1 default=ignore] pam_unix.so\n",
        ).await;

        let result = FileBlockExecutor.execute("chk-1", &json!({
            "path": path,
            "must_contain": ["pam_pwquality", "minlen=15"],
            "match_mode": "literal"
        })).await.unwrap();

        assert!(result.passed);
    }

    #[tokio::test]
    async fn fails_when_required_pattern_missing() {
        let dir = tempdir().unwrap();
        let path = write_file(
            dir.path(), "common-password",
            "password requisite pam_pwquality.so retry=3 minlen=8\n",
        ).await;

        let result = FileBlockExecutor.execute("chk-1", &json!({
            "path": path,
            "must_contain": ["minlen=15"],
            "match_mode": "literal"
        })).await.unwrap();

        assert!(!result.passed);
        assert!(result.detail.contains("minlen=15"));
    }

    #[tokio::test]
    async fn fails_when_forbidden_pattern_present() {
        let dir = tempdir().unwrap();
        let path = write_file(
            dir.path(), "common-auth",
            "auth sufficient pam_unix.so nullok\n",
        ).await;

        let result = FileBlockExecutor.execute("chk-1", &json!({
            "path": path,
            "must_not_contain": ["nullok"],
            "match_mode": "literal"
        })).await.unwrap();

        assert!(!result.passed);
        assert!(result.detail.contains("nullok"));
    }

    #[tokio::test]
    async fn works_with_regex_patterns() {
        let dir = tempdir().unwrap();
        let path = write_file(
            dir.path(), "sshd_config",
            "PermitRootLogin no\nPasswordAuthentication no\n",
        ).await;

        let result = FileBlockExecutor.execute("chk-1", &json!({
            "path": path,
            "must_contain": [
                r"PermitRootLogin\s+no",
                r"PasswordAuthentication\s+no"
            ],
            "must_not_contain": [r"PermitEmptyPasswords\s+yes"],
            "match_mode": "regex"
        })).await.unwrap();

        assert!(result.passed);
    }

    #[tokio::test]
    async fn case_insensitive_literal_match() {
        let dir = tempdir().unwrap();
        let path = write_file(dir.path(), "config", "PermitRootLogin NO\n").await;

        let result = FileBlockExecutor.execute("chk-1", &json!({
            "path": path,
            "must_contain": ["permitrootlogin no"],
            "match_mode": "literal",
            "case_insensitive": true
        })).await.unwrap();

        assert!(result.passed);
    }

    #[tokio::test]
    async fn error_on_empty_patterns() {
        let dir = tempdir().unwrap();
        let path = write_file(dir.path(), "file", "content").await;

        let result = FileBlockExecutor.execute("chk-1", &json!({
            "path": path,
            "must_contain": [],
            "must_not_contain": []
        })).await;

        assert!(result.is_err());
    }
}
