//! DevDock diagnostics and bug reporting system.
//!
//! This crate provides functionality for collecting system diagnostics,
//! creating bug reports, and submitting them to Jira with Slack notifications.

pub mod archive;
pub mod config;
pub mod docker;
pub mod jira;
pub mod report;
pub mod slack;
pub mod startup;
pub mod system;
pub mod tilt;

pub use report::BugReporter;
pub use startup::{collect_startup_diagnostics, StartupDiagnostics};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum DiagnosticsError {
    #[error("Failed to collect system information: {0}")]
    SystemInfo(String),

    #[error("Failed to capture Tilt snapshot: {0}")]
    TiltSnapshot(String),

    #[error("Failed to create archive: {0}")]
    Archive(String),

    #[error("Failed to interact with Jira: {0}")]
    Jira(String),

    #[error("Failed to post to Slack: {0}")]
    Slack(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
}

pub type Result<T> = std::result::Result<T, DiagnosticsError>;
