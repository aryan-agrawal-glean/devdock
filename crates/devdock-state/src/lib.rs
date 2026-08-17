//! State management for DevDock instances.

pub mod db;
pub mod forward;
pub mod instance;
mod migration;
pub mod paths;
pub mod ports;
pub mod session;

pub use db::StateManager;
pub use forward::{PortMappingRecord, DEVDOCK_FORWARD_OWNER, LOOPBACK_BIND_ADDRESS};
pub use instance::{Instance, InstanceStatus, SessionOrigin};
pub use paths::StatePaths;
pub use ports::PortAllocator;
pub use session::{
    ClaimedRemoteSession, RemoteSessionRequest, SessionManifestV1, SessionPortMappingV1,
};
