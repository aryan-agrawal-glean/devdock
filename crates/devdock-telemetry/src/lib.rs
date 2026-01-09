//! DevDock Telemetry Plugin
//!
//! Captures comprehensive observability data for DevDock sessions and emits
//! to BigQuery for analytics and monitoring.
//!
//! ## Data Captured
//!
//! - **Session metrics**: Start/end times, duration, status
//! - **Resource metrics**: Per-resource health, bootup time, status transitions
//! - **Host metrics**: CPU, memory, disk usage
//! - **Docker metrics**: Per-container resource usage
//! - **Build metrics**: Bazel build times, success/failure
//! - **Failure logs**: Log dumps for persistently failing resources

mod bigquery;
mod collector;
mod emitter;
mod error;
mod models;

pub use bigquery::{BigQueryConfig, BigQueryEmitter};
pub use collector::{TelemetryCollector, TelemetryCollectorConfig};
pub use emitter::TelemetryEmitter;
pub use error::{Result, TelemetryError};
pub use models::*;
