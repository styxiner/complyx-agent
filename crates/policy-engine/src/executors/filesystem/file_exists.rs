//! Check `file_exists`: verifica existencia, tipo, propietario y permisos de un path.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::CheckResult;
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
    fn check_type(&self) -> &'static str {
        "file_exists"
    }

    async fn execute (&self, check_id: &str, params: &serde_json::Value,) -> Result<EngineCheckResult, CheckError> {
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

        // verifica el tipo
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
                        file_type_str(expected_type)
                    ),
                ));
            }
        }

        // Verificaciones Unix (owner, group y modo)
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
                            "'{}' propietario = '{}' (esperado {})",
                            p.path.display(),
                            actual_owner,
                            expected_owner
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
                            expected_group
                        ),
                    ));
                }
            }

            if let Some(expected_mode) = &p.mode {
                let actual_mode = format!("{:04o}", meta.mode() & 0o7777);
                // Normalizamos: "600" → "0600"
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
                            expected_norm
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
