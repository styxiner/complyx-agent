//! # policy-engine
//!
//! Librería que interpreta y ejecuta checks de política en el sistema local.
//!
//! Es la pieza central del agente: recibe un `PolicyBundle` del servidor (a través
//! de `grpc-client`) y produce `CheckResult`s para enviar de vuelta.
//!
//! ## Características
//!
//! - **Sin I/O de red**: todas las operaciones son locales (ficheros, procesos del sistema).
//! - **Sin shell**: los executors nunca ejecutan strings arbitrarios. Cada tipo de check
//!   tiene su propia implementación compilada.
//! - **Extensible**: añadir un nuevo tipo de check es crear un fichero e implementar
//!   `CheckExecutor`.
//! - **Testeable de forma aislada**: al no tener dependencias de red, se puede testear
//!   con ficheros temporales sin necesidad de servidor.
//!
//! ## Uso
//!
//! ```no_run
//! use policy_engine::PolicyEngine;
//! use proto::PolicyBundle;
//!
//! #[tokio::main]
//! async fn main() {
//!     let engine = PolicyEngine::new();
//!
//!     // Al arrancar, loguear los tipos soportados
//!     tracing::info!(
//!         types = ?engine.supported_check_types(),
//!         "policy-engine inicializado"
//!     );
//!
//!     // En el poll_loop, tras recibir un bundle nuevo:
//!     let bundle: PolicyBundle = todo!("recibido del servidor");
//!     let results = engine.run_all(&bundle).await;
//!
//!     // results es Vec<proto::CheckResult>, listo para encolar en local-db
//! }
//! ```

mod engine;
mod executor;
mod executors;
mod result;

// API pública
pub use engine::PolicyEngine;
pub use executor::CompareOperator;
pub use result::{CheckError, EngineCheckResult};
