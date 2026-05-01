//! Check `ini_value`: verifica el valor de una clave en un fichero con formato INI.
//!
//! Soporta el formato `[Section]\nkey = value` que usan ficheros como
//! `/etc/security/pwquality.conf`, `smb.conf` o cualquier fichero `.ini`.
//!
//! ## Comportamiento de secciones
//!
//! - `section: null` — busca la clave en el bloque global (antes de cualquier `[Section]`).
//! - `section: "global"` — busca en la sección `[global]`.
//! - Si la misma clave aparece varias veces en la misma sección, se usa la última
//!   ocurrencia (comportamiento habitual en parsers INI: la última gana).
//!
//! ## Ejemplo de uso
//!
//! ```json
//! {
//!   "type": "ini_value",
//!   "path": "/etc/security/pwquality.conf",
//!   "section": null,
//!   "key": "minlen",
//!   "operator": ">=",
//!   "value": "15"
//! }
//! ```

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::{CheckExecutor, CompareOperator};
use crate::result::{CheckError, EngineCheckResult};

pub struct IniValueExecutor;

#[derive(Deserialize)]
struct Params {
    path: PathBuf,

    #[serde(default)]
    section: Option<String>, // Si es `null`, es el bloque global (antes de cualqueir `[Section]`)

    key: String,

    operator: CompareOperator,

    value: String,

    #[serde(default = "default_true")]
    required: bool, // falla si la clave no existe (true)

    #[serde(default = "default_separator")]
    separator: char, // Separador entre clave y valor. Por defecto es `=`, pero depende del fichero
                     // :c
}

fn default_true() -> bool {
    true 
}

fn default_separator() -> char {
    '=' 
}

#[async_trait]
impl CheckExecutor for IniValueExecutor {
    fn check_type(&self) -> &'static str { "ini_value" }

    async fn execute(&self, check_id: &str, params: &serde_json::Value,) -> Result<EngineCheckResult, CheckError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| CheckError::invalid_params("ini_value", e))?;

        let content = tokio::fs::read_to_string(&p.path)
            .await
            .map_err(|e| CheckError::io(p.path.display().to_string(), e))?;

        let actual = find_ini_value(&content, p.section.as_deref(), &p.key, p.separator);

        match actual {
            Some(found_value) => {
                let passed = p.operator.compare(&found_value, &p.value);
                let expected_str = format!("{} {}", p.operator, p.value);
                let detail = EngineCheckResult::value_detail(
                    &p.key, &found_value, &p.operator, &p.value, passed,
                );

                Ok(if passed {
                    EngineCheckResult::pass(check_id, &found_value, &expected_str, detail)
                } else {
                    EngineCheckResult::fail(check_id, &found_value, &expected_str, detail)
                })
            }
            None if p.required => Ok(EngineCheckResult::fail(
                check_id,
                "ausente",
                &format!("{} {}", p.operator, p.value),
                format!("clave '{}' no encontrada en sección '{}' de '{}'", p.key, p.section.as_deref().unwrap_or("<global>"), p.path.display(),),
            )),
            None => Ok(EngineCheckResult::pass(
                check_id,
                "ausente",
                "no requerida",
                format!("clave '{}' no encontrada en '{}' (no requerida)", p.key, p.path.display(),),
            )),
        }
    }
}

// Busca el valor de `key` en la sección `section` del contenido INI.
//
// Si `section` es `None`, busca en el bloque global (líneas antes del primer `[Section]`).
// Si la clave aparece varias veces, devuelve la última ocurrencia.
fn find_ini_value(content: &str, section: Option<&str>, key: &str, separator: char) -> Option<String> {
    let target_section = section.map(|s| s.to_lowercase());
    let mut current_section: Option<String> = None; // None = bloque global ACUERDATE
    let mut found: Option<String> = None;
    let key_lower = key.to_lowercase();

    for line in content.lines() {
        let trimmed = line.trim();

        // Ignorar líneas vacías y comentarios (# y ;)
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }

        // Detectar inicio de sección: [SectionName]
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let name = trimmed[1..trimmed.len() - 1].trim().to_lowercase();
            current_section = Some(name);
            continue;
        }

        // Verificar si estamos en la sección correcta
        let in_target = match (&target_section, &current_section) {
            (None, None) => true, // global
            (Some(t), Some(c)) => t == c, // sección nombrada
            _ => false,
        };

        if !in_target {
            continue;
        }

        // Parsear la línea como key = value
        if let Some(sep_pos) = trimmed.find(separator) {
            let line_key = trimmed[..sep_pos].trim().to_lowercase();
            if line_key == key_lower {
                let value = trimmed[sep_pos + 1..].trim().to_string();
                found = Some(value); // La última ocurrencia gana
            }
        }
    }

    found
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
    async fn passes_for_global_key_gte() {
        let dir = tempdir().unwrap();
        let path = write_file(
            dir.path(), "pwquality.conf",
            "# pwquality config\nminlen = 15\ndifok = 3\n",
        ).await;

        let result = IniValueExecutor.execute("chk-1", &json!({
            "path": path,
            "section": null,
            "key": "minlen",
            "operator": ">=",
            "value": "15"
        })).await.unwrap();

        assert!(result.passed);
        assert_eq!(result.actual_value, "15");
    }

    #[tokio::test]
    async fn fails_for_global_key_too_low() {
        let dir = tempdir().unwrap();
        let path = write_file(dir.path(), "pwquality.conf", "minlen = 8\n").await;

        let result = IniValueExecutor.execute("chk-1", &json!({
            "path": path,
            "key": "minlen",
            "operator": ">=",
            "value": "15"
        })).await.unwrap();

        assert!(!result.passed);
        assert_eq!(result.actual_value, "8");
    }

    #[tokio::test]
    async fn finds_key_in_named_section() {
        let dir = tempdir().unwrap();
        let path = write_file(
            dir.path(), "smb.conf",
            "[global]\nworkgroup = WORKGROUP\nserver signing = mandatory\n\
             [homes]\ncomment = Home Directories\n",
        ).await;

        let result = IniValueExecutor.execute("chk-1", &json!({
            "path": path,
            "section": "global",
            "key": "server signing",
            "operator": "=",
            "value": "mandatory"
        })).await.unwrap();

        assert!(result.passed);
    }

    #[tokio::test]
    async fn does_not_find_key_in_wrong_section() {
        let dir = tempdir().unwrap();
        let path = write_file(
            dir.path(), "smb.conf",
            "[homes]\nkey = value\n[global]\nother = data\n",
        ).await;

        // Busca "key" en la sección global, pero está en "homes"
        let result = IniValueExecutor.execute("chk-1", &json!({
            "path": path,
            "section": "global",
            "key": "key",
            "operator": "=",
            "value": "value",
            "required": true
        })).await.unwrap();

        assert!(!result.passed);
        assert!(result.detail.contains("no encontrada"));
    }

    #[tokio::test]
    async fn last_occurrence_wins() {
        let dir = tempdir().unwrap();
        // La segunda aparición de minlen sobreescribe la primera
        let path = write_file(
            dir.path(), "pwquality.conf",
            "minlen = 8\nminlen = 20\n",
        ).await;

        let result = IniValueExecutor.execute("chk-1", &json!({
            "path": path,
            "key": "minlen",
            "operator": ">=",
            "value": "15"
        })).await.unwrap();

        assert!(result.passed);
        assert_eq!(result.actual_value, "20");
    }

    #[tokio::test]
    async fn key_lookup_is_case_insensitive() {
        let dir = tempdir().unwrap();
        let path = write_file(dir.path(), "pwquality.conf", "MinLen = 16\n").await;

        let result = IniValueExecutor.execute("chk-1", &json!({
            "path": path,
            "key": "minlen",
            "operator": ">=",
            "value": "15"
        })).await.unwrap();

        assert!(result.passed);
    }

    #[test]
    fn find_ini_value_global() {
        let content = "key1 = val1\n[section]\nkey1 = val2\n";
        assert_eq!(
            find_ini_value(content, None, "key1", '='),
            Some("val1".to_string())
        );
    }

    #[test]
    fn find_ini_value_in_section() {
        let content = "key1 = val1\n[section]\nkey1 = val2\n";
        assert_eq!(
            find_ini_value(content, Some("section"), "key1", '='),
            Some("val2".to_string())
        );
    }
}
