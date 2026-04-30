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
    // * Solo 
}
