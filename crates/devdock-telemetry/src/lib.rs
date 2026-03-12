//! DevDock Telemetry — Collection + Local Emission
//!
//! Captures observability data for DevDock sessions.
//! Telemetry SYNC to external services (BigQuery, Datadog, etc.)
//! is handled by external plugins.
//!
//! Built-in emitters:
//! - `FileEmitter` — writes to local JSON file
//! - `NoopEmitter` — discards (for testing)

mod collector;
mod emitter;
mod error;
mod models;

pub use collector::{TelemetryCollector, TelemetryCollectorConfig};
pub use emitter::{FileEmitter, TelemetryEmitter};
pub use error::{Result, TelemetryError};
pub use models::*;
