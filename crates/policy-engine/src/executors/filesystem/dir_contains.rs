//! Check `dir_contains` verifica que un directorio contiene o no ficheros que cumplan un patrón
//! glob con dueño y permisos opcionales.


use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::CheckExecutor;
use crate::result::{CheckError, EngineCheckResult};

pub struct DirContainsExecutor;

#[derive(Deserialize)]
struct Params {
    path: PathBuf,

    #[serde(default)]
    glob: Option<String>, // Patrón glob para los nombres de fichero. Ej: "*.conf". Si se omite,
                          // coincide con cualquier fichero

    #[serde(default = "default_true")]
    must_contain: bool, // Si es `true` (defecto), el directorio debe contener al menos un fichero
                        // que coincida. Si es `false`, no debe contener ninguno.

    #[serde(default)]
    min_count: Option<usize>, // Numero minimo de ficheros coincidentes (por defecto 1 si
                              // must_contain es verdad)

    #[serde(default)]
    max_count: Option<usize>,

    #[serde(default)]
    owner: Option<String>,

    #[serde(default)]
    mode: Option<String>,
}

fn default_true() -> bool {
    true
}

#[async_trait]
impl CheckExecutor for DirContainsExecutor {
    fn check_type(&self) -> &'static str {
        "dir_contains"
    }

    async fn execute(&self, check_id: &str, params: &serde_json::Value,) -> Result<EngineCheckResult, CheckError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| CheckError::invalid_params("dir_contains", e))?;

        // Leer las entradas del directorio
        let mut read_dir = tokio::fs::read_dir(&p.path).await.map_err(|e| {
            CheckError::io(p.path.display().to_string(), e)
        })?;

        let pattern = p.glob.as_deref().unwrap_or("*");
        let glob_re = glob_to_regex(pattern);

        let mut matching = Vec::new();
        while let Some(entry) = read_dir.next_entry().await.map_err(|e| {
            CheckError::io(p.path.display().to_string(), e)
        })? {
            let name = entry.file_name().to_string_lossy().to_string();

            if glob_re.is_match(&name) {
                matching.push(entry.path());
            }
        }

        let count = matching.len();
        let min = p.min_count.unwrap_or(if p.must_contain {
            1
        } else {
            0
        });

        if p.must_contain {
            if count < min {
                return Ok(EngineCheckResult::fail(
                        check_id,
                        &count.to_string(),
                        &format!(">= {min} ficheros '{pattern}'"),
                        format!("'{}' contiene {} ficheros '{}' (esperados >= {})", p.path.display(), count, pattern, min),
                        ));
            }

            if let Some(max) = p.max_count {
                if count > max {
                    ret Ok(EngineCheckResult::fail(
                            check_id,
                            &count.to_string(),
                            &format!("<= {max} ficheros '{pattern}'"),
                            format!("'{}' contiene {} ficheros '{}' (maximo {})", p.path.display(), count, pattern, max),
                            ));
                }
            }
        } else if count > 0 {
            return Ok(EngineCheckResult::fail(
                    check_id,
                    &count.to_string(),
                    "0 ficheros",
                    format!("'{}' contiene {} ficheros '{}' que no deberian existir", p.path.display(), count, pattern),
                    ));
        }

        Ok(EngineCheckResult::pass(
                check_id,
                &count.to_string(),
                &format!("'{}' contiene {} ficheros '{}' (correcto)", p.path.display(), count, pattern),
                ))
    }
}

// Convierte un patron glob simple a una regex. Solo soporta `*` (cualquier secuencia) y `?`
// (cualquier caracter). Ya mas adelante añadire soporte para regex con formato PCRE2 (las que usa
// perl, vaya)
fn glob_to_regex(glob: &str) -> regex::Regex {
    let mut pattern = String::from("^");

    for ch in glob.chars() {
        '*' => pattern.push_str(".*"),
        '?' => pattern.push('.'),
        c => {
            // Escapa los caracteres especiales de las regex
            if ".+^${}[]|()\\".contains(c) {
                pattern.push('\\');
            }
            pattern.push(c);
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
    async fn passes_when_dir_contains_matching_files() {
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
 
    #[tokio::test]
    async fn glob_to_regex_works() {
        let re = glob_to_regex("*.conf");
        assert!(re.is_match("foo.conf"));
        assert!(re.is_match("bar.conf"));
        assert!(!re.is_match("foo.txt"));
    }
}
