//! Tipo `EngineCheckResult` interno del policy-engine y tipo de error `CheckError`.

use crate::executor::CompareOperator;

/// Error que puede producir un executor.
#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("parámetros inválidos para check '{check_type}': {reason}")]
    InvalidParams { check_type: String, reason: String },

    #[error("no se pudo leer '{path}': {reason}")]
    IoError { path: String, reason: String },

    #[error("permisos insuficientes para acceder a '{path}'")]
    PermissionDenied { path: String },

    #[error("tipo de check '{0}' no soportado por esta versión del agente")]
    UnsupportedType(String),

    #[error("error interno en check '{check_type}': {reason}")]
    Internal { check_type: String, reason: String },
}

impl CheckError {
    pub fn invalid_params(check_type: &str, err: serde_json::Error) -> Self {
        Self::InvalidParams {
            check_type: check_type.to_string(),
            reason: err.to_string(),
        }
    }

    pub fn io(path: impl Into<String>, err: std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::PermissionDenied {
            return Self::PermissionDenied { path: path.into() };
        }
        Self::IoError {
            path: path.into(),
            reason: err.to_string(),
        }
    }
}

/// Resultado interno del engine, más rico que el tipo proto.
/// Se convierte a `proto::CheckResult` en `engine.rs`.
#[derive(Debug, Clone)]
pub struct EngineCheckResult {
    pub check_id: String,
    pub passed: bool,
    pub detail: String,
    pub actual_value: String,
    pub expected_value: String,
}

impl EngineCheckResult {
    pub fn pass(
        check_id: impl Into<String>,
        actual: impl Into<String>,
        expected: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            check_id: check_id.into(),
            passed: true,
            detail: detail.into(),
            actual_value: actual.into(),
            expected_value: expected.into(),
        }
    }

    pub fn fail(
        check_id: impl Into<String>,
        actual: impl Into<String>,
        expected: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            check_id: check_id.into(),
            passed: false,
            detail: detail.into(),
            actual_value: actual.into(),
            expected_value: expected.into(),
        }
    }

    pub fn value_detail(
        key: &str,
        actual: &str,
        op: &CompareOperator,
        expected: &str,
        passed: bool,
    ) -> String {
        if passed {
            format!("{key} = {actual} (satisface {op} {expected})")
        } else {
            format!("{key} = {actual} (esperado {op} {expected})")
        }
    }
}

impl From<EngineCheckResult> for proto::CheckResult {
    fn from(r: EngineCheckResult) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let executed_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

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

    #[test]
    fn pass_result() {
        let r = EngineCheckResult::pass("chk-1", "15", ">= 15", "OK");
        assert!(r.passed);
        assert_eq!(r.check_id, "chk-1");
    }

    #[test]
    fn fail_result() {
        let r = EngineCheckResult::fail("chk-2", "8", ">= 15", "insuficiente");
        assert!(!r.passed);
    }

    #[test]
    fn value_detail_pass() {
        let d = EngineCheckResult::value_detail(
            "PASS_MIN_LEN", "15", &CompareOperator::Gte, "15", true,
        );
        assert!(d.contains("satisface"));
    }

    #[test]
    fn value_detail_fail() {
        let d = EngineCheckResult::value_detail(
            "PASS_MIN_LEN", "8", &CompareOperator::Gte, "15", false,
        );
        assert!(d.contains("esperado"));
        assert!(d.contains("8"));
    }

    #[test]
    fn from_engine_result_to_proto() {
        let r: proto::CheckResult = EngineCheckResult::pass("chk-1", "15", ">= 15", "OK").into();
        assert!(r.passed);
        assert_eq!(r.check_id, "chk-1");
        assert!(r.executed_at > 0);
    }
}
