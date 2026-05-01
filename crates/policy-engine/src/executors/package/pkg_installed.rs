//! Check `pkg_installed`: verifica que un paquete está instalado con restricción de versión.
//!
//! Detecta automáticamente el gestor de paquetes del sistema (dpkg, rpm, pacman)
//! o permite forzarlo explícitamente. No ejecuta comandos de shell arbitrarios:
//! solo llama a los binarios específicos de cada gestor con argumentos fijos.
//!
//! ## Ejemplo de uso
//!
//! ```json
//! {
//!   "type": "pkg_installed",
//!   "name": "openssh-server",
//!   "version": "8.9",
//!   "operator": ">=",
//!   "package_manager": "auto"
//! }
//! ```

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::{CheckExecutor, CompareOperator};
use crate::result::{CheckError, EngineCheckResult};

pub struct PkgInstalledExecutor;

#[derive(Deserialize, Default, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PackageManager {
    #[default]
    Auto,

    Dpkg, // Sistemas Debian/Ubuntu: `dpkg-query`
    Rpm, // Sistemas RHEL/Fedora/SUSE: `rpm`
    Pacman, // Arch Linux: `pacman`
}

#[derive(Deserialize)]
struct Params {
    name: String,

    #[serde(default)]
    version: Option<String>, // Versión esperada. Si se omite, solo verifica que el paquete está instalado.

    #[serde(default = "default_operator")]
    operator: CompareOperator, // Operador para comparar versiones. Solo aplica si `version` está presente.

    #[serde(default)]
    package_manager: PackageManager, // Gestor de paquetes a usar. `"auto"` detecta automáticamente.

}

fn default_operator() -> CompareOperator { CompareOperator::Eq }

#[async_trait]
impl CheckExecutor for PkgInstalledExecutor {
    fn check_type(&self) -> &'static str { "pkg_installed" }

    async fn execute(&self, check_id: &str, params: &serde_json::Value,) -> Result<EngineCheckResult, CheckError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| CheckError::invalid_params("pkg_installed", e))?;

        let pm = resolve_package_manager(&p.package_manager).await;

        let installed_version = query_package(&pm, &p.name).await.map_err(|e| {
            CheckError::Internal {
                check_type: "pkg_installed".into(),
                reason: e,
            }
        })?;

        match installed_version {
            None => Ok(EngineCheckResult::fail(
                check_id,
                "no instalado",
                p.version.as_deref().unwrap_or("cualquier versión"),
                format!("paquete '{}' no está instalado", p.name),
            )),
            Some(actual_ver) => {
                match &p.version {
                    None => Ok(EngineCheckResult::pass(
                        check_id,
                        &actual_ver,
                        "instalado",
                        format!("paquete '{}' instalado (versión {})", p.name, actual_ver),
                    )),
                    Some(expected_ver) => {
                        // Comparar versiones usando el operador configurado.
                        // Para versiones de paquetes uso una comparación semántica simplificada:
                        // primero intentamos numérica, si falla comparamos lexicográficamente.
                        let passed = compare_versions(&p.operator, &actual_ver, expected_ver);
                        let expected_str = format!("{} {}", p.operator, expected_ver);
                        let detail = EngineCheckResult::value_detail(
                            &p.name, &actual_ver, &p.operator, expected_ver, passed,
                        );

                        Ok(if passed {
                            EngineCheckResult::pass(check_id, &actual_ver, &expected_str, detail)
                        } else {
                            EngineCheckResult::fail(check_id, &actual_ver, &expected_str, detail)
                        })
                    }
                }
            }
        }
    }
}

// Determina el gestor de paquetes a usar.
// En modo `Auto`, prueba en orden: dpkg → rpm → pacman.
async fn resolve_package_manager(pm: &PackageManager) -> PackageManager {
    if *pm != PackageManager::Auto {
        return pm.clone();
    }

    // Detectar por presencia de binarios
    for (bin, variant) in &[
        ("/usr/bin/dpkg-query", PackageManager::Dpkg),
        ("/usr/bin/dpkg",       PackageManager::Dpkg),
        ("/usr/bin/rpm",        PackageManager::Rpm),
        ("/usr/bin/pacman",     PackageManager::Pacman),
    ] {
        if tokio::fs::metadata(bin).await.is_ok() {
            return variant.clone();
        }
    }

    // Fallback a dpkg (Debian es el más común en servidores)
    PackageManager::Dpkg
}

// Consulta si `pkg_name` está instalado y devuelve su versión.
// Devuelve `None` si el paquete no está instalado.
async fn query_package(pm: &PackageManager, pkg_name: &str) -> Result<Option<String>, String> {
    let (program, args) = match pm {
        PackageManager::Dpkg | PackageManager::Auto => (
            "dpkg-query",
            vec!["-W", "-f=${Version}", pkg_name],
        ),
        PackageManager::Rpm => (
            "rpm",
            vec!["-q", "--queryformat", "%{VERSION}-%{RELEASE}", pkg_name],
        ),
        PackageManager::Pacman => (
            "pacman",
            vec!["-Q", pkg_name],
        ),
    };

    let output = tokio::process::Command::new(program)
        .args(&args)
        .output() // Sin shell: los argumentos se pasan directamente al binario
        .await
        .map_err(|e| format!("no se pudo ejecutar '{}': {}", program, e))?;

    if !output.status.success() {
        // El paquete no está instalado (exit code != 0 en todos los gestores)
        return Ok(None);
    }

    let version_raw = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if version_raw.is_empty() {
        return Ok(None);
    }

    // extraer la version de "nombre versión" de pac man
    let version = if *pm == PackageManager::Pacman {
        version_raw
            .split_whitespace()
            .nth(1)
            .unwrap_or(&version_raw)
            .to_string()
    } else {
        version_raw
    };

    Ok(Some(version))
}

// Compara dos versiones de paquete usando el operador dado.
//
// 1. Intenta extraer el primer componente numérico (major version) y comparar numéricamente.
// 2. Si falla (versiones no numéricas), compara lexicográficamente.
//
// Esto cubre la mayoria de casos de uso reales. Para comparar versiones completas como semver o
// debian/rpm habría que hacer otra librería (la cual ni de coña la hago ahora) la cual añadiría
// dependencias innecesarias para la mayoria de chequeos.
fn compare_versions(op: &CompareOperator, actual: &str, expected: &str) -> bool {
    // Extraer solo los dígitos iniciales para comparación numérica del major
    let actual_num = extract_version_number(actual);
    let expected_num = extract_version_number(expected);

    match (actual_num, expected_num) {
        (Some(a), Some(e)) => {
            // Comparación de versiones numéricas componente a componente
            let passed_str = format!("{}", a);
            let expected_str = format!("{}", e);
            op.compare(&passed_str, &expected_str)
        }
        _ => {
            // Fallback: comparación lexicográfica
            op.compare(actual, expected)
        }
    }
}

// Extrae el número de versión principal (antes del primer `-`, `+`, `~` o letra).
// Ejemplo: "2.9.1-1ubuntu3" → Some(2.91) no es preciso; mejor: comparamos por partes.
fn extract_version_number(version: &str) -> Option<f64> {
    // Tomamos solo los caracteres que forman la versión base: dígitos y puntos
    let base: String = version
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();

    if base.is_empty() {
        return None;
    }

    // Comparamos solo el major.minor para la comparación numérica
    let parts: Vec<u64> = base
        .split('.')
        .filter_map(|p| p.parse().ok())
        .collect();

    if parts.is_empty() {
        return None;
    }

    // Convertimos a un float representativo: major.minor (ej. 8.9 → 8.9)
    let major = parts[0] as f64;
    let minor = parts.get(1).copied().unwrap_or(0) as f64;
    Some(major + minor * 0.001) // Evita colisiones: 8.9 < 8.10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_versions_eq() {
        assert!(compare_versions(&CompareOperator::Eq, "8.9", "8.9"));
        assert!(!compare_versions(&CompareOperator::Eq, "8.9", "8.10"));
    }

    #[test]
    fn compare_versions_gte() {
        assert!(compare_versions(&CompareOperator::Gte, "9.0", "8.9"));
        assert!(compare_versions(&CompareOperator::Gte, "8.9", "8.9"));
        assert!(!compare_versions(&CompareOperator::Gte, "8.8", "8.9"));
    }

    #[test]
    fn extract_version_number_basic() {
        assert!(extract_version_number("8.9").is_some());
        assert!(extract_version_number("2.9.1-1ubuntu3").is_some());
        assert!(extract_version_number("abc").is_none());
    }
}
