//! Port allocation for DevDock instances.

use anyhow::Result;

/// Manages port allocation for DevDock instances.
pub struct PortAllocator {
    base_port: u16,
    next_offset: u16,
}

impl PortAllocator {
    /// Create a new port allocator.
    pub fn new() -> Self {
        Self {
            base_port: 7960,
            next_offset: 0,
        }
    }

    /// Allocate a port base for a new instance.
    pub fn allocate(&mut self) -> Result<u16> {
        let port = self.base_port + (self.next_offset * 100);
        self.next_offset += 1;
        Ok(port)
    }

    /// Check if a port is available.
    pub fn is_port_available(port: u16) -> bool {
        use std::net::TcpListener;
        TcpListener::bind(("127.0.0.1", port)).is_ok()
    }
}

impl Default for PortAllocator {
    fn default() -> Self {
        Self::new()
    }
}

