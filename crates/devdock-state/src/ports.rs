//! Port allocation for DevDock instances.

use crate::{InstanceStatus, StateManager};
use anyhow::Result;

/// Manages port allocation for DevDock instances.
pub struct PortAllocator {
    state_manager: StateManager,
}

impl PortAllocator {
    /// Create a new port allocator with database connection.
    pub async fn new(state_manager: StateManager) -> Self {
        Self { state_manager }
    }

    /// Allocate a port offset for a new instance.
    ///
    /// Finds the first available offset starting from 0, in increments of 1000.
    /// Only considers active (Running/Starting) instances when determining available offsets.
    pub async fn allocate(&self) -> Result<u16> {
        let instances = self.state_manager.list_instances().await?;

        // Get all port offsets currently in use by active instances
        let used_offsets: Vec<u16> = instances
            .iter()
            .filter(|i| matches!(i.status, InstanceStatus::Running | InstanceStatus::Starting))
            .map(|i| i.port_base)
            .collect();

        // Find first available offset starting from 0, in increments of 1000
        for offset in (0..65000).step_by(1000) {
            if !used_offsets.contains(&offset) {
                return Ok(offset);
            }
        }

        anyhow::bail!("No available port offsets")
    }

    /// Check if a port is available.
    pub fn is_port_available(port: u16) -> bool {
        use std::net::TcpListener;
        TcpListener::bind(("127.0.0.1", port)).is_ok()
    }
}
