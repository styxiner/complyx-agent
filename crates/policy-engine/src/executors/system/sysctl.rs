//! Check `sysctl`: verifica el valor de un parámetro del kernel en tiempo de ejecución.
//!
//! Lee directamente de `/proc/sys/` convirtiendo el nombre del parámetro
//! (`net.ipv4.ip_forward` → `/proc/sys/net/ipv4/ip_forward`), sin ejecutar el binario
//! `sysctl` para evitar dependencias del PATH.
//!
//! ## Ejemplo de uso
//!
//! ```json
//! {
//!   "type": "sysctl",
//!   "key": "net.ipv4.ip_forward",
//!   "operator": "=",
//!   "value": "0"
//! }
//! ```

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::{CheckExecutor, CompareOperator};
use crate::result::{CheckError, EngineCheckResult};

pub struct SysctlExecutor;

#[derive(Deserialize)]
struct Params {
    /// Nombre del parámetro en notación con puntos (ej. `net.ipv4.ip_forward`).
    key: String,

    /// Operador de comparación.
    operator: CompareOperator,

    /// Valor esperado.
    value: String,
}

#[async_trait]
impl CheckExecutor for SysctlExecutor {
    fn check_type(&self) -> &'static str { "sysctl" }

    async fn execute(
        &self,
        check_id: &str,
        params: &serde_json::Value,
    ) -> Result<EngineCheckResult, CheckError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| CheckError::invalid_params("sysctl", e))?;

        let proc_path = sysctl_key_to_proc_path(&p.key);

        let raw = tokio::fs::read_to_string(&proc_path)
            .await
            .map_err(|e| CheckError::io(proc_path.clone(), e))?;

        let actual = raw.trim().to_string();
        let passed = p.operator.compare(&actual, &p.value);
        let expected_str = format!("{} {}", p.operator, p.value);
        let detail = EngineCheckResult::value_detail(&p.key, &actual, &p.operator, &p.value, passed);

        Ok(if passed {
            EngineCheckResult::pass(check_id, &actual, &expected_str, detail)
        } else {
            EngineCheckResult::fail(check_id, &actual, &expected_str, detail)
        })
    }
}

/// Convierte `net.ipv4.ip_forward` → `/proc/sys/net/ipv4/ip_forward`.
fn sysctl_key_to_proc_path(key: &str) -> String {
    format!("/proc/sys/{}", key.replace('.', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_to_proc_path_conversion() {
        assert_eq!(
            sysctl_key_to_proc_path("net.ipv4.ip_forward"),
            "/proc/sys/net/ipv4/ip_forward"
        );
        assert_eq!(
            sysctl_key_to_proc_path("kernel.randomize_va_space"),
            "/proc/sys/kernel/randomize_va_space"
        );
    }

    // Solo ejecutamos este test en Linux donde /proc/sys existe
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn reads_real_kernel_parameter() {
        // kernel.hostname siempre existe en Linux
        let result = SysctlExecutor
            .execute("chk-1", &json!({
                "key": "kernel.hostname",
                "operator": "!=",
                "value": ""
            }))
            .await
            .unwrap();

        // El hostname siempre es no vacío en un sistema real
        assert!(result.passed, "kernel.hostname debería ser no vacío: {}", result.detail);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn checks_ip_forward_exists() {
        let result = SysctlExecutor
            .execute("chk-1", &json!({
                "key": "net.ipv4.ip_forward",
                "operator": "=",
                "value": "0"
            }))
            .await;

        // El check puede pasar o fallar según el sistema, pero no debe dar error
        assert!(result.is_ok(), "no debería dar error: {:?}", result.err());
    }
}
