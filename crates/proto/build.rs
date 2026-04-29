//! Generación de código rust a partir de complyx.proto
//!
//! Este script se ejecuta por Cargo antes de compilar el crate.
//! Invoca tonic-build, que usa protoc para:
//! 1. Generar structs Rust de cada mensaje Protobuf via prost
//! 2. Genera los traits e implementaciones de los servicios gRPC via tonic
//!
//! El codigo generado se escribe en $OUT_DIR/complyx.rs y se incluye desde lib.rs
//!
//! IMPORTANTE:
//! Esto necesita requisitos adicionales para funcionar
//!
//! `protoc`: El compilador de Protocol Buffers debe estar instalado y disponible en el PATH.
//! Instalación:
//! Debian: sudo apt install -y protobuf-compiler
//! RHEL: sudo dnf install -y protobuf-compiler
//!
//! Configuración del agente vs servidor
//! Este crate pertenece al AGENTE:
//! * build_server(false): no genera los traits de servidor
//! * build_client(true): genera los stubs de cliente para ambos servicios
//!
//! El crate proto del servidor tiene la configuración inversa:
//! * build_server(true), build_client(false)

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false)
        .build_client(true)

        // Añadir derives adicionales a los structs generados para poder clonarlos. Necesario en
        // retry.rs donde los requests se clonan en cada reintento y serializarlos con serde para
        // debug y persistencia en local-db.
        .type_attribute(".", "#[derive(Clone)]")
        // Rutas: Ficheros .proto y directorios donde buscar imports
        .compile(
            &["proto/complyx.proto"], //fichero a compilar
            &["proto"], // directorios para importar entre protos
        )?;

    // Indicar a cargo que recompile este crate si el .proto cambia. Sin esto, los cambios en el
    // .proto no desencadenarán recompilación (malo ;p )
    println!("cargo:rerun-if-changed=proto/complyx.proto");

    Ok(())
}
