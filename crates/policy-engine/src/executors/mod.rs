//! Registro de todos los executors disponibles en el policy-engine.
//!
//! `register_all_executors()` es el único lugar donde se instancian los executors
//! y se mapean a sus `check_type`. Añadir un nuevo executor implica:
//!
//! 1. Crear el fichero en el subdirectorio correspondiente.
//! 2. Implementar el trait `CheckExecutor`.
//! 3. Añadir una línea en `register_all_executors()`.
//! 4. Exportarlo en el `mod.rs` de su categoría.

pub mod content;
pub mod filesystem;
pub mod package;
pub mod system;

use std::collections::HashMap;
use std::sync::Arc;

use crate::executor::CheckExecutor;

/// Construye un `HashMap` con todos los executors registrados, indexados por `check_type`.
/// Se llama una sola vez al inicializar el `PolicyEngine`.
pub fn register_all_executors() -> HashMap<&'static str, Arc<dyn CheckExecutor>> {
    let mut map: HashMap<&'static str, Arc<dyn CheckExecutor>> = HashMap::new();

    // Filesystem
    map.insert("file_exists",  Arc::new(filesystem::FileExistsExecutor));
    map.insert("file_absent",  Arc::new(filesystem::FileAbsentExecutor));
    map.insert("dir_contains", Arc::new(filesystem::DirContainsExecutor));
    map.insert("symlink",      Arc::new(filesystem::SymlinkExecutor));

    // Content
    map.insert("file_line",    Arc::new(content::FileLineExecutor));
    map.insert("file_block",   Arc::new(content::FileBlockExecutor));
    map.insert("ini_value",    Arc::new(content::IniValueExecutor));

    // Package
    map.insert("pkg_installed", Arc::new(package::PkgInstalledExecutor));
    map.insert("pkg_absent",    Arc::new(package::PkgAbsentExecutor));

    // System
    map.insert("sysctl",       Arc::new(system::SysctlExecutor));
    map.insert("service",      Arc::new(system::ServiceExecutor));
    map.insert("user_attr",    Arc::new(system::UserAttrExecutor));

    map
}
