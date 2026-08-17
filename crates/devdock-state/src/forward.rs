//! Durable identity for DevDock-managed port forwards.

use serde::{Deserialize, Serialize};

pub const DEVDOCK_FORWARD_OWNER: &str = "devdock";
pub const LOOPBACK_BIND_ADDRESS: &str = "127.0.0.1";

/// Complete database representation of a port mapping.
///
/// Ownership fields remain optional so schema-0/older writers can continue to
/// write rows. A live row with incomplete identity is observable, but it is
/// never sufficient authority for DevDock to terminate a process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortMappingRecord {
    pub devdock_id: String,
    pub service_name: String,
    pub port_type: String,
    pub local_port: u16,
    pub remote_port: u16,
    pub process_pid: Option<u32>,
    pub workspace_id: Option<String>,
    pub owner_kind: Option<String>,
    pub process_start_time: Option<u64>,
    pub command_fingerprint: Option<String>,
}

impl PortMappingRecord {
    pub fn is_devdock_managed(&self) -> bool {
        self.owner_kind.as_deref() == Some(DEVDOCK_FORWARD_OWNER)
            && self.process_pid.is_some()
            && self.workspace_id.is_some()
            && self.process_start_time.is_some()
            && self.command_fingerprint.is_some()
    }
}
