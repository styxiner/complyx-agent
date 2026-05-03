pub mod file_block_set;
pub mod file_line_set;
pub mod pkg_install;
pub mod pkg_remove;
pub mod service_set;
pub mod sysctl_set;

pub use file_block_set::FileBlockSetRemediator;
pub use file_line_set::FileLineSetRemediator;
pub use pkg_install::PkgInstallRemediator;
pub use pkg_remove::PkgRemoveRemediator;
pub use service_set::ServiceSetRemediator;
pub use sysctl_set::SysctlSetRemediator;

use std::collections::HashMap;
use std::sync::Arc;

use crate::executor::RemediationExecutor;

// Construye el registro de todos los `remediators`, indexados por su `remediation_type`. Se llama
// una sola vez al inicializar el `RemediationEngine`
pub fn register_all_remediators() -> HashMap<&'static str, Arc<dyn RemediationExecutor>> {
    let mut map: HashMap<&'static str, Arc<dyn RemediationExecutor>> = HashMap::new();

    map.insert("file_line_set",  Arc::new(FileLineSetRemediator));
    map.insert("file_block_set", Arc::new(FileBlockSetRemediator));
    map.insert("pkg_install",    Arc::new(PkgInstallRemediator));
    map.insert("pkg_remove",     Arc::new(PkgRemoveRemediator));
    map.insert("sysctl_set",     Arc::new(SysctlSetRemediator));
    map.insert("service_set",    Arc::new(ServiceSetRemediator));

    map
}
