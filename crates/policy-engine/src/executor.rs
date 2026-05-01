//! Trait base para todos los executors de checks y tipo `CompareOperator`.

use async_trait::async_trait;
use serde::Deserialize;

use crate::result::{CheckError, EngineCheckResult};

/// Interfaz que debe implementar cada tipo de check.
#[async_trait]
pub trait CheckExecutor: Send + Sync {
    fn check_type(&self) -> &'static str;

    async fn execute(
        &self,
        check_id: &str,
        params: &serde_json::Value,
    ) -> Result<EngineCheckResult, CheckError>;
}

/// Operadores de comparación disponibles en los checks de tipo valor.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CompareOperator {
    #[serde(rename = "=")]
    Eq,
    #[serde(rename = "!=")]
    Ne,
    #[serde(rename = ">=")]
    Gte,
    #[serde(rename = "<=")]
    Lte,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = "<")]
    Lt,
    Contains,
    NotContains,
    Regex,
}

impl CompareOperator {
    pub fn compare(&self, actual: &str, expected: &str) -> bool {
        match self {
            Self::Eq => actual == expected,
            Self::Ne => actual != expected,
            Self::Contains => actual.contains(expected),
            Self::NotContains => !actual.contains(expected),
            Self::Regex => {
                regex::Regex::new(expected)
                    .map(|re| re.is_match(actual))
                    .unwrap_or(false)
            }
            op => {
                let a: f64 = match actual.trim().parse() {
                    Ok(v) => v,
                    Err(_) => return false,
                };
                let e: f64 = match expected.trim().parse() {
                    Ok(v) => v,
                    Err(_) => return false,
                };
                match op {
                    Self::Gte => a >= e,
                    Self::Lte => a <= e,
                    Self::Gt  => a > e,
                    Self::Lt  => a < e,
                    _ => unreachable!(),
                }
            }
        }
    }

    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Eq          => "=",
            Self::Ne          => "!=",
            Self::Gte         => ">=",
            Self::Lte         => "<=",
            Self::Gt          => ">",
            Self::Lt          => "<",
            Self::Contains    => "contains",
            Self::NotContains => "not_contains",
            Self::Regex       => "regex",
        }
    }
}

impl std::fmt::Display for CompareOperator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.symbol())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eq_operator() {
        assert!(CompareOperator::Eq.compare("15", "15"));
        assert!(!CompareOperator::Eq.compare("14", "15"));
    }

    #[test]
    fn gte_numeric() {
        assert!(CompareOperator::Gte.compare("16", "15"));
        assert!(CompareOperator::Gte.compare("15", "15"));
        assert!(!CompareOperator::Gte.compare("14", "15"));
    }

    #[test]
    fn gte_non_numeric_returns_false() {
        assert!(!CompareOperator::Gte.compare("abc", "15"));
    }

    #[test]
    fn contains_operator() {
        assert!(CompareOperator::Contains.compare("hello world", "world"));
        assert!(!CompareOperator::Contains.compare("hello world", "xyz"));
    }

    #[test]
    fn regex_valid() {
        assert!(CompareOperator::Regex.compare("/bin/bash", r"^/bin/(bash|sh)$"));
        assert!(!CompareOperator::Regex.compare("/bin/zsh", r"^/bin/(bash|sh)$"));
    }

    #[test]
    fn regex_invalid_pattern_returns_false() {
        assert!(!CompareOperator::Regex.compare("anything", "[invalid regex"));
    }
}
