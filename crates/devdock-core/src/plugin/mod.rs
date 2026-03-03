//! Configuration-driven backend system.
//!
//! Backends are NOT plugins users write — they're built-in options
//! selected via `devdock.toml` configuration. The backend system
//! provides trait interfaces so the pipeline code works identically
//! regardless of which backend is active.
//!
//! ## Backend Types
//! - [`ConfigStorageBackend`] — local filesystem, GCS, S3
//! - [`TelemetrySyncBackend`] — stdout, BigQuery, Datadog
//! - [`DiagnosticsBackend`] — stdout, Jira, GitHub Issues
//! - [`RemoteProviderBackend`] — SSH, Coder, Codespaces

pub mod external;
mod registry;
mod traits;

pub use external::ExternalTelemetryPlugin;
pub use registry::BackendRegistry;
pub use traits::*;
