//! Error types for the monitor crate.

use thiserror::Error;

/// Result type alias for monitor operations.
pub type Result<T> = std::result::Result<T, MonitorError>;

/// Errors that can occur during monitoring operations.
#[derive(Error, Debug)]
pub enum MonitorError {
    #[error("Tilt command failed: {0}")]
    TiltCommand(String),

    #[error("Failed to parse Tilt output: {0}")]
    ParseError(String),

    #[error("Tilt is not running")]
    TiltNotRunning,

    #[error("Startup timeout after {0} seconds")]
    Timeout(u64),

    #[error("Resource '{0}' failed to become healthy")]
    ResourceFailed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}
