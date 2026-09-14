//! Error types for the monitor crate.

use thiserror::Error;

/// Result type alias for monitor operations.
pub type Result<T> = std::result::Result<T, MonitorError>;

/// Errors that can occur during monitoring operations.
#[derive(Error, Debug)]
pub enum MonitorError {
    #[error("Tilt command failed: {0}")]
    TiltCommand(String),

    #[error("Invalid Tilt endpoint: {0}")]
    InvalidTiltEndpoint(String),

    #[error("Tilt HTTP request failed: {0}")]
    TiltHttp(String),

    #[error("Tilt /api/view returned HTML or a non-JSON response ({0})")]
    TiltViewHtmlResponse(String),

    #[error("Tilt /api/view schema is incompatible: {0}")]
    TiltViewSchema(String),

    #[error("Tilt web session did not issue a Tilt-Token cookie")]
    TiltWebCookieMissing,

    #[error("Tilt resource '{0}' was not found")]
    TiltTriggerUnknownResource(String),

    #[error("Tilt resource trigger failed: {0}")]
    TiltTrigger(String),

    #[error("Tilt resource '{0}' has no completed build span for this update")]
    TiltTriggerPending(String),

    #[error("Tilt resource '{0}' only exposes a prior build span")]
    TiltTriggerStale(String),

    #[error("Tilt resource '{resource}' build span '{span_id}' failed: {error}")]
    TiltBuildFailed { resource: String, span_id: String, error: String },

    #[error("Tilt resource '{0}' is not ready for this update")]
    TiltResourceNotReady(String),

    #[error("Tilt restarted while collecting evidence")]
    TiltRestarted,

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
