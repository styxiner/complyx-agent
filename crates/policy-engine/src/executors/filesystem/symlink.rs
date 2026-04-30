//! Check `symlink`: verifica que un symlink existe y apunta al target correcto.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::CheckExecutor;
use crate::result::{CheckError, EngineCheckResult};

pub struct SymlinkExecutor;

#[derive(Deserialize)]
struct Params {
    path: PathBuf,

    #[serde(default)]
    target: Option<PathBuf>, // Target al que se debe apuntar el symlink. Si se omite, solo verifca
                             // que el path es un symlink (sin importar el destino, lo cual seria
                             // lo mismo que el check de ficheros)
}

#[async_trait]
impl CheckExecutor for SymlinkExecutor {
    fn check_type(&self) -> &'static str {
        "symlink"
    }
 
    async fn execute(&self, check_id: &str, params: &serde_json::Value,) -> Result<EngineCheckResult, CheckError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| CheckError::invalid_params("symlink", e))?;
 
        // symlinl_metadata no sigue el symlink. Si el path es un symlink, devuelve sus metadatos,
        // no los del destino. Es necesario para verificar que es un symlink vaya :d
        let meta = match tokio::fs::symlink_metadata(&p.path).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(EngineCheckResult::fail(
                    check_id,
                    "ausente",
                    "symlink",
                    format!("'{}' no existe", p.path.display()),
                ));
            }
            Err(e) => return Err(CheckError::io(p.path.display().to_string(), e)),
        };
 
        if !meta.file_type().is_symlink() {
            return Ok(EngineCheckResult::fail(
                check_id,
                "no es symlink",
                "symlink",
                format!("'{}' existe pero no es un symlink (es {})", p.path.display(), if meta.is_file() { 
                    "fichero" 
                } else { 
                    "directorio" 
                }
                ),
            ));
        }
 
        // Verificar el destino si se especificó
        if let Some(expected_target) = &p.target {
            let actual_target = tokio::fs::read_link(&p.path)
                .await
                .map_err(|e| CheckError::io(p.path.display().to_string(), e))?;
 
            if actual_target != *expected_target {
                return Ok(EngineCheckResult::fail(
                    check_id,
                    &actual_target.display().to_string(),
                    &expected_target.display().to_string(),
                    format!("'{}' apunta a '{}' (esperado '{}')", p.path.display(), actual_target.display(), expected_target.display(),),
                ));
            }
 
            return Ok(EngineCheckResult::pass(
                check_id,
                &actual_target.display().to_string(),
                &expected_target.display().to_string(),
                format!("'{}' → '{}' (correcto)", p.path.display(), actual_target.display(),),
            ));
        }
 
        Ok(EngineCheckResult::pass(
            check_id,
            "symlink",
            "symlink",
            format!("'{}' es un symlink", p.path.display()),
        ))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;
 
    #[tokio::test]
    async fn passes_for_symlink_with_correct_target() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real.conf");
        let link = dir.path().join("link.conf");
        tokio::fs::write(&real, b"content").await.unwrap();
        tokio::fs::symlink(&real, &link).await.unwrap();
 
        let result = SymlinkExecutor
            .execute("chk-1", &json!({ "path": link, "target": real }))
            .await
            .unwrap();
 
        assert!(result.passed, "debería pasar: el symlink apunta al target correcto");
    }
 
    #[tokio::test]
    async fn fails_for_wrong_target() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real.conf");
        let other = dir.path().join("other.conf");
        let link = dir.path().join("link.conf");
        tokio::fs::write(&real, b"a").await.unwrap();
        tokio::fs::write(&other, b"b").await.unwrap();
        tokio::fs::symlink(&real, &link).await.unwrap();
 
        let result = SymlinkExecutor
            .execute("chk-1", &json!({ "path": link, "target": other }))
            .await
            .unwrap();
 
        assert!(!result.passed);
        assert!(result.detail.contains("esperado"));
    }
 
    #[tokio::test]
    async fn fails_when_path_is_regular_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("file.txt");
        tokio::fs::write(&file, b"x").await.unwrap();
 
        let result = SymlinkExecutor
            .execute("chk-1", &json!({ "path": file }))
            .await
            .unwrap();
 
        assert!(!result.passed);
        assert!(result.detail.contains("no es symlink"));
    }
 
    #[tokio::test]
    async fn fails_when_absent() {
        let result = SymlinkExecutor
            .execute("chk-1", &json!({ "path": "/tmp/complyx_test_no_existe_symlink" }))
            .await
            .unwrap();
 
        assert!(!result.passed);
        assert!(result.detail.contains("no existe"));
    }
}
 

