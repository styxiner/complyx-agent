//! Remediador `pkg_remove`: desinstala un paquete del sistema.
//!
//! ## Ejemplo de params
//!
//! ```json
//! {
//!   "name": "telnetd",
//!   "purge": true
//! }
//! ```

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::{RemediationError, RemediationExecutor, RemediationResult};
use super::pkg_install::{PackageManager, resolve_pm};

pub struct PkgRemoveRemediator;

#[derive(Deserialize)]
struct Params {
    name: String,

    #[serde(default)]
    purge: bool, // Si `true`, elimina también los ficheros de configuración (solo apt: `purge`).

    #[serde(default)]
    package_manager: PackageManager,
}

#[async_trait]
impl RemediationExecutor for PkgRemoveRemediator {
    fn remediation_type(&self) -> &'static str { "pkg_remove" }

    async fn execute(&self, remediation_id: &str, params: &serde_json::Value,) -> Result<RemediationResult, RemediationError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| RemediationError::invalid_params("pkg_remove", e))?;

        let pm = resolve_pm(&p.package_manager).await;
        let (program, args) = remove_command(&pm, &p.name, p.purge);

        tracing::info!(
            remediation_id,
            package = %p.name,
            purge = p.purge,
            "desinstalando paquete"
        );

        let output = tokio::process::Command::new(program)
            .args(&args)
            .env("DEBIAN_FRONTEND", "noninteractive")
            .output()
            .await
            .map_err(|e| RemediationError::Internal {
                remediation_type: "pkg_remove".into(),
                reason: format!("no se pudo ejecutar '{}': {}", program, e),
            })?;

        if output.status.success() {
            Ok(RemediationResult::applied(format!(
                "paquete '{}' desinstalado correctamente{}",
                p.name,
                if p.purge { " (con purge)" } else { "" }
            )))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(RemediationError::Internal {
                remediation_type: "pkg_remove".into(),
                reason: format!(
                    "fallo al desinstalar '{}' (exit {}): {}",
                    p.name,
                    output.status.code().unwrap_or(-1),
                    stderr
                ),
            })
        }
    }
}

fn remove_command<'a>(pm: &PackageManager, pkg: &'a str, purge: bool) -> (&'static str, Vec<&'a str>) {
    match pm {
        PackageManager::Apt | PackageManager::Auto => {
            if purge {
                ("apt-get", vec!["purge", "-y", pkg])
            } else {
                ("apt-get", vec!["remove", "-y", pkg])
            }
        }
        PackageManager::Dnf => ("dnf",    vec!["remove", "-y", pkg]),
        PackageManager::Yum => ("yum",    vec!["remove", "-y", pkg]),
        PackageManager::Pacman => ("pacman", vec!["--noconfirm", "-R", pkg]),
    }
}
