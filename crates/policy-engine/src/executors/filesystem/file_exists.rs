//! Check `file_exists`: verifica existencia, tipo, propietario y permisos de un path.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::CheckExecutor;
use crate::result::{CheckError, EngineCheckResult};

pub struct FileExistsExecutor;

#[derive(Deserialize)]
struct Params {
    path: PathBuf,

    #[serde(default)]
    file_type: Option<FileType>,

    #[serde(default)]
    owner: Option<String>,

    #[serde(default)]
    group: Option<String>,

    /// Permisos en notación octal como string (ej. "0600", "0644").
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum FileType {
    File,
    Dir,
    Symlink,
}

#[async_trait]
impl CheckExecutor for FileExistsExecutor {
    fn check_type(&self) -> &'static str { "file_exists" }

    async fn execute(
        &self,
        check_id: &str,
        params: &serde_json::Value,
    ) -> Result<EngineCheckResult, CheckError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| CheckError::invalid_params("file_exists", e))?;

        let meta = match tokio::fs::symlink_metadata(&p.path).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(EngineCheckResult::fail(
                    check_id,
                    "ausente",
                    "existente",
                    format!("'{}' no existe", p.path.display()),
                ));
            }
            Err(e) => return Err(CheckError::io(p.path.display().to_string(), e)),
        };

        // Verificar tipo
        if let Some(expected_type) = &p.file_type {
            let actual_type = if meta.is_file() {
                FileType::File
            } else if meta.is_dir() {
                FileType::Dir
            } else {
                FileType::Symlink
            };
            if *expected_type != actual_type {
                return Ok(EngineCheckResult::fail(
                    check_id,
                    file_type_str(&actual_type),
                    file_type_str(expected_type),
                    format!(
                        "'{}' existe pero es {} (esperado {})",
                        p.path.display(),
                        file_type_str(&actual_type),
                        file_type_str(expected_type),
                    ),
                ));
            }
        }

        // Verificaciones Unix (owner, group, mode)
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            if let Some(expected_owner) = &p.owner {
                let uid = meta.uid();
                let actual_owner = uid_to_name(uid).unwrap_or_else(|| uid.to_string());
                if &actual_owner != expected_owner {
                    return Ok(EngineCheckResult::fail(
                        check_id,
                        &actual_owner,
                        expected_owner.as_str(),
                        format!(
                            "'{}' propietario = '{}' (esperado '{}')",
                            p.path.display(),
                            actual_owner,
                            expected_owner,
                        ),
                    ));
                }
            }

            if let Some(expected_group) = &p.group {
                let gid = meta.gid();
                let actual_group = gid_to_name(gid).unwrap_or_else(|| gid.to_string());
                if &actual_group != expected_group {
                    return Ok(EngineCheckResult::fail(
                        check_id,
                        &actual_group,
                        expected_group.as_str(),
                        format!(
                            "'{}' grupo = '{}' (esperado '{}')",
                            p.path.display(),
                            actual_group,
                            expected_group,
                        ),
                    ));
                }
            }

            if let Some(expected_mode) = &p.mode {
                let actual_mode = format!("{:04o}", meta.mode() & 0o7777);
                let expected_norm = normalize_mode(expected_mode);
                if actual_mode != expected_norm {
                    return Ok(EngineCheckResult::fail(
                        check_id,
                        &actual_mode,
                        &expected_norm,
                        format!(
                            "'{}' permisos = {} (esperado {})",
                            p.path.display(),
                            actual_mode,
                            expected_norm,
                        ),
                    ));
                }
            }
        }

        Ok(EngineCheckResult::pass(
            check_id,
            "existente",
            "existente",
            format!("'{}' existe y cumple todos los requisitos", p.path.display()),
        ))
    }
}

fn file_type_str(t: &FileType) -> &'static str {
    match t {
        FileType::File    => "file",
        FileType::Dir     => "dir",
        FileType::Symlink => "symlink",
    }
}

fn normalize_mode(mode: &str) -> String {
    if mode.len() == 3 { format!("0{mode}") } else { mode.to_string() }
}

#[cfg(unix)]
fn uid_to_name(uid: u32) -> Option<String> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in passwd.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 && parts[2].parse::<u32>().ok()? == uid {
            return Some(parts[0].to_string());
        }
    }
    None
}

#[cfg(unix)]
fn gid_to_name(gid: u32) -> Option<String> {
    let group = std::fs::read_to_string("/etc/group").ok()?;
    for line in group.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 && parts[2].parse::<u32>().ok()? == gid {
            return Some(parts[0].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn passes_for_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        tokio::fs::write(&path, b"content").await.unwrap();

        let result = FileExistsExecutor
            .execute("chk-1", &json!({ "path": path, "file_type": "file" }))
            .await
            .unwrap();

        assert!(result.passed);
    }

    #[tokio::test]
    async fn fails_for_missing_file() {
        let result = FileExistsExecutor
            .execute("chk-1", &json!({ "path": "/tmp/complyx_test_no_existe_xyz" }))
            .await
            .unwrap();

        assert!(!result.passed);
        assert!(result.detail.contains("no existe"));
    }

    #[tokio::test]
    async fn fails_when_type_mismatch() {
        let dir = tempdir().unwrap();
        let result = FileExistsExecutor
            .execute("chk-1", &json!({ "path": dir.path(), "file_type": "file" }))
            .await
            .unwrap();

        assert!(!result.passed);
    }
}
