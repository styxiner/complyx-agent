//! Check `file_absent`: verifica que un path no existe en el sistema

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::CheckExecutor;
use crate::result::{CheckError, EngineCheckResult}

pub struct FileAbsentExecutor;

#[derive(Deserialize)]
struct Params {
    path: PathBuf,
}

#[async_trait]
impl CheckExecutor for FileAbsentExecutor {
    fn check_type(&self) -> &'static str {
        "file_absent"
    }

    async fn execute(&self, check_id: &str, params: &serde_json::Value) -> Result<EngineCheckResult, CheckError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| CheckError::invalid_params("file_absent", e))?;

        match tokio::fs::symlink_metadata(&p.path).await {
            Ok(_) => Ok(EngineCheckResult::fail(
                    check_id,
                    "presente",
                    "ausente",
                    format!("'{}' no existe (correcto)", p.path.display()),
                    )),
            Err(e) => Err(CheckError::io(p.path.display().to_string(), e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;
 
    #[tokio::test]
    async fn passes_when_file_absent() {
        let result = FileAbsentExecutor
            .execute("chk-1", &json!({ "path": "/tmp/complyx_test_absent_xyz" }))
            .await
            .unwrap();
        assert!(result.passed);
    }
 
    #[tokio::test]
    async fn fails_when_file_present() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("should_not_exist.txt");
        tokio::fs::write(&path, b"oops").await.unwrap();
 
        let result = FileAbsentExecutor
            .execute("chk-1", &json!({ "path": path }))
            .await
            .unwrap();
        assert!(!result.passed);
    }
}
