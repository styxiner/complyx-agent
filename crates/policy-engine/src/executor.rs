//! Trait base para todos los executors de checks y tipo `CompareOperator`

use async_trait::async_trait;
use serde::Deserialize;

use crate::{CheckError, CheckResult};

// Interfaz que debe implementar cada tipo de check
//
// Cada implementacion vive en su ejecutor: `executors/<categoria>/<tipo>.rs` y se registra eb
// `executors/mod.rs` dentro de `register_all_executors()`.
//
// Los ejecutores son stateless: No guardan estado entre ejecutores. Todo lo que necesiten estará
// en `params`
#[async_trait]
pub trait CheckExecutor: Send + Sync {
    // Identificador del tipo de check. Debe coincidir exactamente con `check_type` del
    // `PolicyCheck` en el proto.
    fn check_type(&self) -> &'static str;

    // Ejecuta el check con los parametros dados y devuelve el resultado.
    //
    // Condiciones
    // * Nunca debe hacer panic. Los errores se devuelven como `Err(CheckError)`
    // * No debe hacer E/S de red 
    // * Solo lee del sistema de ficheros, no escribe
    // * Debe ser reproducible: El mismo estado del sistema produce el mismo resultado
    async fn execute(
        &self,
        check_id: &str,
        params: &serde_json::Value,
    ) -> Result<CheckResult, CheckError>;
}

// Operadores de comparacion disponibles en los checks de tipo valor.
//
// Se usa en `file_line`, `ini_value`, `sysctl` y `user_attr` para comparar el valor encontrado en
// el sistema con el valor esperado en la politica.

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
    // Compara el valor actual con el valor esperado usando este operador.
    //
    // Para los operadores numericos intenta parsear ambos valores como `f64`. Si no son numericos,
    // devuelve `false`
    pub fn compare(&self, actual: &str, expected: &str) -> bool {
        match self {
	        Self::Eq => actual == expected,
	        Self::Ne => actual != expected,
	        Self::Contains => actual.contains(expected),
	        Self::NotContains => !actual.contains(expected),
	        Self::Regex => regex::Regex::new(expected)
	            .map(|e| re.is_match(actual))
	            .unwrap_or(false),
	
	        // Operadores numericos
	        op => {
	            let a:f64 = match actual.trim().parse() {
	                Ok(v) => v,
	                Err(_) => return false,
	            };
	
	            let e: f64 = match expected.trim().parse() {
	                Ok(v) => v,
	                Err(_) => return false,
	            }
	
	            let op {
	                  Self::Gte => a >= e,
	                  Self::Lte => a <= e,
	                  Self::Gt => a > e,
    	              Self::Lt => a < e,
	                  _ => unreachable!(),
                }
            }
        }
    }

    // Representacion legible del operador para los mensajes de resultado
    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Ne => "!=",
            Self::Gte => ">=",
            Self::Lte => "<=",
            Self::Gt => ">",
            Self::Lt => "<",
            Self::Contains => "contains",
            Self::NotContains => "not contains",
            Self::Regex => "regex",
        }
    }
}


impl std::fmt::Display for CompareOperator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Formatter {
        write!(f, "{}", self.symbol())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
 
    #[test]
    fn eq_operator() {
        let op = CompareOperator::Eq;
        assert!(op.compare("15", "15"));
        assert!(!op.compare("14", "15"));
    }
 
    #[test]
    fn gte_operator_numeric() {
        let op = CompareOperator::Gte;
        assert!(op.compare("15", "15"));
        assert!(op.compare("16", "15"));
        assert!(!op.compare("14", "15"));
    }
 
    #[test]
    fn gte_operator_non_numeric_returns_false() {
        let op = CompareOperator::Gte;
        assert!(!op.compare("abc", "15"));
    }
 
    #[test]
    fn contains_operator() {
        let op = CompareOperator::Contains;
        assert!(op.compare("hello world", "world"));
        assert!(!op.compare("hello world", "xyz"));
    }
 
    #[test]
    fn regex_operator() {
        let op = CompareOperator::Regex;
        assert!(op.compare("/bin/bash", r"^/bin/(bash|sh)$"));
        assert!(!op.compare("/bin/zsh", r"^/bin/(bash|sh)$"));
    }
 
    #[test]
    fn regex_invalid_pattern_returns_false() {
        let op = CompareOperator::Regex;
        assert!(!op.compare("anything", "[regex invalida"));
    }
}
