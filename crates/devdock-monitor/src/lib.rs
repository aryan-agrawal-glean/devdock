//! DevDock startup monitoring and resource health tracking.
//!
//! This crate provides functionality to monitor Tilt resources during DevDock startup,
//! waiting for all resources to become healthy or timing out after a configurable duration.

mod error;
mod health_tracker;
mod notification;
mod resource;
mod session;
mod startup;
mod stats;
mod tilt;

pub use error::{MonitorError, Result};
pub use health_tracker::{HealthTracker, StabilityConfig, StabilityInfo};
pub use notification::Notifier;
pub use resource::{ResourceLabel, ResourceStatus, ResourceType, TiltResource};
pub use session::{SessionState, SessionTiming};
pub use startup::{StartupMonitor, StartupMonitorConfig, StartupResult};
pub use stats::{HostStats, ResourceStats, StatsCollector};
pub use tilt::TiltClient;

