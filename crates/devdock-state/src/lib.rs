//! State management for DevDock instances.

pub mod db;
pub mod instance;
mod migration;
pub mod paths;
pub mod ports;
pub mod session;

pub use db::StateManager;
pub use instance::{Instance, InstanceStatus, SessionOrigin};
pub use paths::StatePaths;
pub use ports::PortAllocator;
pub use session::{ClaimedRemoteSession, RemoteSessionRequest, SessionManifestV1};
