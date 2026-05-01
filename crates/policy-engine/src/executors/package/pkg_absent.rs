//! Check `pkg_absent`: verifica que un paquete NO está instalado en el sistema.
//!
//! Útil para hardening: paquetes de acceso remoto inseguro (telnet, rsh),
//! compiladores en servidores de producción, etc.
//!
//! ## Ejemplo de uso
//!
//! ```json
//! {
//!   "type": "pkg_absent",
//!   "name": "telnetd",
//!   "reason": "CIS Benchmark 2.1.1: telnet server must not be installed"
//! }
//! ```

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::CheckExecutor;
use crate::result::{CheckError, EngineCheckResult};
use super::pkg_installed::{PackageManager, PkgInstalledExecutor};

pub struct PkgAbsentExecutor;

#[derive(Deserialize)]
struct Params {
    name: String,

    #[serde(default)]
    reason: Option<String>, // Por temas de auditoria

    #[serde(default)]
    package_manager: PackageManager, // Gestor de paquetes. `"auto"` detecta automáticamente.
}

#[async_trait]
impl CheckExecutor for PkgAbsentExecutor {
    fn check_type(&self) -> &'static str { "pkg_absent" }

    async fn execute(&self, check_id: &str, params: &serde_json::Value,) -> Result<EngineCheckResult, CheckError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| CheckError::invalid_params("pkg_absent", e))?;

        // Reutilizamos la lógica de consulta de pkg_installed
        let installed_params = serde_json::json!({
            "name": p.name,
            "package_manager": p.package_manager,
        });

        // Ejecutamos pkg_installed internamente para ver si está instalado
        let installed_result = PkgInstalledExecutor
            .execute("_internal", &installed_params)
            .await?;

        if installed_result.actual_value == "no instalado" || !installed_result.passed {
            // El paquete no está instalado: correcto
            Ok(EngineCheckResult::pass(
                check_id,
                "no instalado",
                "no instalado",
                format!("paquete '{}' no está instalado (correcto)", p.name),
            ))
        } else {
            // El paquete está instalado. Fallo
            let reason_note = p
                .reason
                .as_deref()
                .map(|r| format!(" — {}", r))
                .unwrap_or_default();

            Ok(EngineCheckResult::fail(
                check_id,
                &format!("instalado ({})", installed_result.actual_value),
                "no instalado",
                format!("paquete '{}' está instalado pero no debería{}", p.name, reason_note),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn passes_for_nonexistent_package() {
        // Este paquete no debería existir en ningún sistema de CI
        let result = PkgAbsentExecutor
            .execute("chk-1", &json!({
                "name": "complyx-test-nonexistent-package-xyz-9999"
            }))
            .await
            .unwrap();

        assert!(result.passed);
    }
}
