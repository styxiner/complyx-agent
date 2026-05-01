//! Check `pkg_absent`: verifica que un paquete NO esta instalado en el sistema.

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::CheckExecutor;
use crate::result::{CheckError, EngineCheckResult};
use super::pkg_installed::{PackageManager, query_package, resolve_package_manager};

pub struct PkgAbsentExecutor;

#[derive(Deserialize)]
struct Params {
    name: String,

    #[serde(default)]
    reason: Option<String>,

    #[serde(default)]
    package_manager: PackageManager,
}

#[async_trait]
impl CheckExecutor for PkgAbsentExecutor {
    fn check_type(&self) -> &'static str { "pkg_absent" }

    async fn execute(
        &self,
        check_id: &str,
        params: &serde_json::Value,
    ) -> Result<EngineCheckResult, CheckError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| CheckError::invalid_params("pkg_absent", e))?;

        let pm = resolve_package_manager(&p.package_manager).await;

        let installed = query_package(&pm, &p.name)
            .await
            .map_err(|e| CheckError::Internal {
                check_type: "pkg_absent".into(),
                reason: e,
            })?;

        match installed {
            None => Ok(EngineCheckResult::pass(
                check_id,
                "no instalado",
                "no instalado",
                format!("paquete '{}' no esta instalado (correcto)", p.name),
            )),
            Some(version) => {
                let reason_note = p
                    .reason
                    .as_deref()
                    .map(|r| format!(" — {}", r))
                    .unwrap_or_default();

                Ok(EngineCheckResult::fail(
                    check_id,
                    &format!("instalado ({})", version),
                    "no instalado",
                    format!(
                        "paquete '{}' esta instalado pero no deberia{}",
                        p.name, reason_note,
                    ),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn passes_for_nonexistent_package() {
        let result = PkgAbsentExecutor
            .execute("chk-1", &json!({
                "name": "complyx-test-nonexistent-package-xyz-9999"
            }))
            .await
            .unwrap();

        assert!(result.passed);
    }
}
