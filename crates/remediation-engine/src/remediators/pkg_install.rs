//! Remediador `pkg_install`: instala un paquete mediante el gestor del sistema.
//!
//! Detecta automáticamente `apt-get`, `dnf`/`yum` o `pacman` según el sistema.
//! Invoca el gestor con flags no-interactivos para que no bloquee el agente.
//!
//! ## Ejemplo de params
//!
//! ```json
//! {
//!   "name": "auditd",
//!   "package_manager": "auto"
//! }
//! ```

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::{RemediationError, RemediationExecutor, RemediationResult};

pub struct PkgInstallRemediator;

#[derive(Deserialize, Default, PartialEq, Clone)]
#[serde(rename_all = "lowercase")]
pub enum PackageManager {
    #[default]
    Auto,
    Apt,
    Dnf,
    Yum,
    Pacman,
}

#[derive(Deserialize)]
struct Params {
    name: String,

    #[serde(default)]
    version: Option<String>, // Si es None instala la ultima

    #[serde(default)]
    package_manager: PackageManager,
}

#[async_trait]
impl RemediationExecutor for PkgInstallRemediator {
    fn remediation_type(&self) -> &'static str { 
        "pkg_install" 
    }

    async fn execute(&self, remediation_id: &str, params: &serde_json::Value,) -> Result<RemediationResult, RemediationError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| RemediationError::invalid_params("pkg_install", e))?;

        let pm = resolve_pm(&p.package_manager).await;

        let pkg_spec = match &p.version {
            Some(v) => format_pkg_spec(&pm, &p.name, v),
            None => p.name.clone(),
        };

        let (program, args) = install_command(&pm, &pkg_spec);

        tracing::info!(
            remediation_id,
            package = %pkg_spec,
            program,
            "instalando paquete"
        );

        let output = tokio::process::Command::new(program)
            .args(&args)
            .env("DEBIAN_FRONTEND", "noninteractive") // para apt: evita prompts interactivos
            .output()
            .await
            .map_err(|e| RemediationError::Internal {
                remediation_type: "pkg_install".into(),
                reason: format!("no se pudo ejecutar '{}': {}", program, e),
            })?;

        if output.status.success() {
            Ok(RemediationResult::applied(format!(
                "paquete '{}' instalado correctamente",
                pkg_spec
            )))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(RemediationError::Internal {
                remediation_type: "pkg_install".into(),
                reason: format!(
                    "fallo al instalar '{}' (exit {}): {}",
                    pkg_spec,
                    output.status.code().unwrap_or(-1),
                    stderr
                ),
            })
        }
    }
}

// con pub(super) hace publica la funcion solo al modulo padre inmediato (remediation-engine)
// manteniendo el principio de menor privilegio. Comprobar que los tests pasen
pub(super) async fn resolve_pm(pm: &PackageManager) -> PackageManager {
    if *pm != PackageManager::Auto {
        return pm.clone();
    }
    for (bin, variant) in &[ // Lo hago así para evitar copias innecesarias según el sistema de
                             // ownership del compilador
        ("/usr/bin/apt-get", PackageManager::Apt),
        ("/usr/bin/dnf",     PackageManager::Dnf),
        ("/usr/bin/yum",     PackageManager::Yum),
        ("/usr/bin/pacman",  PackageManager::Pacman),
    ] {
        if tokio::fs::metadata(bin).await.is_ok() {
            return variant.clone();
        }
    }
    PackageManager::Apt
}

fn install_command<'a>(pm: &PackageManager, pkg: &'a str) -> (&'static str, Vec<&'a str>) {
    match pm {
        PackageManager::Apt | PackageManager::Auto => {
            ("apt-get", vec!["install", "-y", "--no-install-recommends", pkg])
        }
        PackageManager::Dnf => ("dnf", vec!["install", "-y", pkg]),
        PackageManager::Yum => ("yum", vec!["install", "-y", pkg]),
        PackageManager::Pacman => ("pacman", vec!["--noconfirm", "-S", pkg]),
    }
}

// Formatea el spec de paquete con versión según el gestor.
// Apt: `paquete=versión`, dnf/yum: `paquete-versión`, pacman: `paquete`
fn format_pkg_spec(pm: &PackageManager, name: &str, version: &str) -> String {
    match pm {
        PackageManager::Apt | PackageManager::Auto => format!("{}={}", name, version),
        PackageManager::Dnf | PackageManager::Yum  => format!("{}-{}", name, version),
        PackageManager::Pacman => name.to_string(), // pacman no soporta versión en install
    }
}
