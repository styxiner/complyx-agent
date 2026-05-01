//! Check `dir_contains`: verifica que un directorio contiene ficheros que cumplan un patrón glob.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::CheckExecutor;
use crate::result::{CheckError, EngineCheckResult};

pub struct DirContainsExecutor;

#[derive(Deserialize)]
struct Params {
    path: PathBuf,

    /// Patrón glob para los nombres de fichero (ej. "*.conf"). Sin glob = cualquier fichero.
    #[serde(default)]
    glob: Option<String>,

    /// Si `true` (defecto), el directorio debe contener al menos uno. Si `false`, ninguno.
    #[serde(default = "default_true")]
    must_contain: bool,

    #[serde(default)]
    min_count: Option<usize>,

    #[serde(default)]
    max_count: Option<usize>,
}

fn default_true() -> bool { true }

#[async_trait]
impl CheckExecutor for DirContainsExecutor {
    fn check_type(&self) -> &'static str { "dir_contains" }

    async fn execute(
        &self,
        check_id: &str,
        params: &serde_json::Value,
    ) -> Result<EngineCheckResult, CheckError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| CheckError::invalid_params("dir_contains", e))?;

        let mut read_dir = tokio::fs::read_dir(&p.path)
            .await
            .map_err(|e| CheckError::io(p.path.display().to_string(), e))?;

        let pattern = p.glob.as_deref().unwrap_or("*");
        let glob_re = glob_to_regex(pattern);

        let mut count = 0usize;
        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| CheckError::io(p.path.display().to_string(), e))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            if glob_re.is_match(&name) {
                count += 1;
            }
        }

        let min = p.min_count.unwrap_or(if p.must_contain { 1 } else { 0 });

        if p.must_contain {
            if count < min {
                return Ok(EngineCheckResult::fail(
                    check_id,
                    &count.to_string(),
                    &format!(">= {min} ficheros '{pattern}'"),
                    format!(
                        "'{}' contiene {} ficheros '{}' (esperado >= {})",
                        p.path.display(), count, pattern, min,
                    ),
                ));
            }
            if let Some(max) = p.max_count {
                if count > max {
                    return Ok(EngineCheckResult::fail(
                        check_id,
                        &count.to_string(),
                        &format!("<= {max} ficheros '{pattern}'"),
                        format!(
                            "'{}' contiene {} ficheros '{}' (máximo {})",
                            p.path.display(), count, pattern, max,
                        ),
                    ));
                }
            }
        } else if count > 0 {
            return Ok(EngineCheckResult::fail(
                check_id,
                &count.to_string(),
                "0 ficheros",
                format!(
                    "'{}' contiene {} fichero(s) '{}' que no deberían existir",
                    p.path.display(), count, pattern,
                ),
            ));
        }

        Ok(EngineCheckResult::pass(
            check_id,
            &count.to_string(),
            &format!(">= {min}"),
            format!(
                "'{}' contiene {} fichero(s) '{}' (correcto)",
                p.path.display(), count, pattern,
            ),
        ))
    }
}

/// Convierte un patrón glob simple a regex. Solo soporta `*` y `?`.
fn glob_to_regex(glob: &str) -> regex::Regex {
    let mut pattern = String::from("^");
    for ch in glob.chars() {
        match ch {
            '*' => pattern.push_str(".*"),
            '?' => pattern.push('.'),
            c => {
                if r".+^${}[]|()\".contains(c) {
                    pattern.push('\\');
                }
                pattern.push(c);
            }
        }
    }
    pattern.push('$');
    regex::Regex::new(&pattern).unwrap_or_else(|_| regex::Regex::new(".*").unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn passes_when_matching_files_found() {
        let dir = tempdir().unwrap();
        tokio::fs::write(dir.path().join("foo.conf"), b"").await.unwrap();
        tokio::fs::write(dir.path().join("bar.conf"), b"").await.unwrap();
        tokio::fs::write(dir.path().join("readme.txt"), b"").await.unwrap();

        let result = DirContainsExecutor
            .execute("chk-1", &json!({ "path": dir.path(), "glob": "*.conf", "min_count": 2 }))
            .await
            .unwrap();

        assert!(result.passed);
    }

    #[tokio::test]
    async fn fails_when_no_matching_files() {
        let dir = tempdir().unwrap();

        let result = DirContainsExecutor
            .execute("chk-1", &json!({ "path": dir.path(), "glob": "*.conf" }))
            .await
            .unwrap();

        assert!(!result.passed);
    }

    #[test]
    fn glob_star_matches_any_extension() {
        let re = glob_to_regex("*.conf");
        assert!(re.is_match("foo.conf"));
        assert!(!re.is_match("foo.txt"));
    }

    #[test]
    fn glob_star_matches_all() {
        let re = glob_to_regex("*");
        assert!(re.is_match("anything.txt"));
        assert!(re.is_match("no_extension"));
    }
}
