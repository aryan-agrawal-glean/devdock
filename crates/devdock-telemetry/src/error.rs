//! Error types for telemetry operations.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, TelemetryError>;

#[derive(Error, Debug)]
pub enum TelemetryError {
    #[error("BigQuery error: {0}")]
    BigQuery(String),

    #[error("Collection error: {0}")]
    Collection(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Monitor error: {0}")]
    Monitor(#[from] devdock_monitor::MonitorError),
}

