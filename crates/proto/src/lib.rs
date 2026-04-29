//! Crate de tipos generados a partir de `complyx.proto`
//!
//! Expone todos los mensajes Protobuf y los stubs de cliente gRPC bajo el modulo `complyx`. El
//! codigo real esta generado por `tonic-build` en compilacion y esta en `$OUT_DIR/complyx.rs`.
//!
//!
//! Los tipos generados aqui son el contrato (descripción del protocolo) de red entre agente y servidor .
//! Los crates de lógica de negocio (`policy-engine`, `remediation-engine`, `result-ingester`...)
//! **no deben depender de este crate directamente**: trabajan con sus propios tipos de dominio.
//! Solo `grpc-client` (en el agente) y `grpc-service` (en el servidor) importan este crate y hacen
//! la conversion entre tipos proto y tipos de dominio.


//! Modulo que contiene todos los tipos generados a partir de `complyx.proto`.
//!
//! El nombre del modulo coincide con el `package complyx;` declarado en el proto.

pub mod complyx {
    // tonic-build escribe el codigo generado en $OUT_DIR/complyx.rs durante compilación. Este
    // include! lo incorpora al crate en compilacion
    tonic::include_proto!("complyx");
}

// Permite importar los tipos mas usados directamente desde `proto::` en lugar de tener que
// escribir `proto::complyx::` en cada import. Solo re-exportará los tipos que los crates
// consumidos necesitan con frecuencia
pub use complyx::{
    CheckResult,
    EnrollRequest,
    EnrollResponse,
    HeartbeatRequest,
    HeartbeatResponse,
    Policy,
    PolicyBundle,
    PolicyCheck,
    PolicyElement,
    PolicyRemediation,
    PollRequest,
    PollResponse,
    SubmitResultsRequest,
    SubmitResultsResponse,
};
