//! Tipo `CheckResult` del engine y helpers para construirlo.
//!
//! Este es el `CheckResult` interno del policy-engine, mas verboso que el tipo proto. El engine lo
//! usa para construir mensajes destrictivos.
//!
//! `engine.rs` lo convierte al tipo proto antes de devolverlo a quien lo llama (caller).

use crate::executor::CompareOperator;

// Error que puede producir un executor al intentar ejecutar un chequeo
#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("parametros invalidos para check '{check_type}': {reason}")]
    InvalidParams {check_type: String, reason: String} // Los parametros JSON del check no se pudieron deserializar al tipo esperado

    #[error("no se puede leer '{path}': {reason}")]
    IoError {path: String, reason: String},

    #[error("permisos insuficientes para acceder a '{path}'")]
    PermissionDenied {path: String},

    #[error("el tipo de check '{0}' no soportado por esta version del agente")]
    UnsupportedType(String),

    #[error("error interno del chequeo '{check_type}': {reason}")]
    Internal {check_type: String, reason: String},
}

impl Checkerror {
    // Construye un `InvalidParams` a partir de `serde_json`
    pub fn invalid_params(check_type: &str, err: serde_json::Error) -> Self {
        Self::InvalidParams {
            check_type: check_type.to_string(),
            reason: err.to_string(),
        }
    }

    // Construye un `IoError` a partir de `std::io::Error`
    pub fn io(path: impl Into<String>, err: std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::PermissionDenied {
            return Self::PermissionDenied {path: path.into()};
        }

        Self::IoError {
            path: path.into(),
            reason: err.to_string(),
        }
    }
}

// Resultado de un check ejecutado por el policuy-engine
//
// Se convierte al tipo proto `CheckResult` en `engine.rs` antes de devolverlo al poll_loop para
// enviarlo al servidor.
#[derive(Debug, Clone)]
pub struct EngineCheckResult {
    pub check_id: String,
    pub passed: bool,
    pub detail: String,
    pub actual_value: String,
    pub expected_value: String,
}

impl EngineCheckResult {
    // El sistema cumple la condicion si el resultado es positivo
    pub fn pass(check_id: impl Into<String>, actual: impl Into<String>, expected: impl Into<String>, detail: impl Into<String>,) -> Self {
        Self {
            check_id: check_id.into(),
            passed: true,
            detail: detail.into(),
            actual_value: detail.into(),
            expected_value: expected.into(),
        }
    }

    // El resultado es negativo si el sistema no cumple la condicion
    pub fn fail(check_id: impl Into<String>, actual: impl Into<String>, expected: impl Into<String>, detail: impl Into<String>,) -> Self {
        Self {
            check_id: check_id.into(),
            passed: false,
            detail: detail.into(),
            actual_value: detail.into(),
            expected_value: expected.into(),
        }
    }

    // Construye el detail estandar para checks de tipo clave:valor. 
    // Ej: `PASS_MIN_LEN = 8` (esperado >= 15)
    pub fn value_detail(key: &str, actual: &str, op: &CompareOperator, expected: &str, passed: bool,) -> String {
        if passed {
            format!("{key} = {actual} (satisface {op} {expected})")
        } else {
            format!("{key} = {actual} (esperado ({op} {expected})")
        }
    }

}

// Convierte el resultado interno del engine al tipo proto `CheckResult`
impl From<EngineCheckResult> for proto::CheckResult {
    fn from(r: EngineCheckResult) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let executed_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as f64;

        proto::CheckResult {
            check_id: r.check_id,
            passed: r.passed,
            detail: r.detail,
            actual_value: r.actual_value,
            expected_value: r.expected_value,
            executed_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::CompareOperator;
 
    #[test]
    fn pass_result_is_marked_passed() {
        let r = EngineCheckResult::pass("chk-1", "15", ">= 15", "OK");
        assert!(r.passed);
        assert_eq!(r.check_id, "chk-1");
    }
 
    #[test]
    fn fail_result_is_not_passed() {
        let r = EngineCheckResult::fail("chk-2", "8", ">= 15", "insuficiente");
        assert!(!r.passed);
    }
 
    #[test]
    fn value_detail_pass() {
        let detail = EngineCheckResult::value_detail(
            "PASS_MIN_LEN",
            "15",
            &CompareOperator::Gte,
            "15",
            true,
        );
        assert!(detail.contains("satisface"));
        assert!(detail.contains("PASS_MIN_LEN"));
    }
 
    #[test]
    fn value_detail_fail() {
        let detail = EngineCheckResult::value_detail(
            "PASS_MIN_LEN",
            "8",
            &CompareOperator::Gte,
            "15",
            false,
        );
        assert!(detail.contains("esperado"));
        assert!(detail.contains("8"));
    }
 
    #[test]
    fn from_engine_result_to_proto() {
        let engine_r = EngineCheckResult::pass("chk-1", "15", ">= 15", "OK");
        let proto_r: proto::CheckResult = engine_r.into();
        assert!(proto_r.passed);
        assert_eq!(proto_r.check_id, "chk-1");
        assert!(proto_r.executed_at > 0);
    }
}
