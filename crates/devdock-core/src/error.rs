//! Error types for DevDock.

use std::path::PathBuf;
use thiserror::Error;

/// Result type alias for DevDock operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Main error type for DevDock.
#[derive(Error, Debug)]
pub enum Error {
    #[error("Profile not found: {0}")]
    ProfileNotFound(PathBuf),

    #[error("Invalid profile: {0}")]
    InvalidProfile(String),

    #[error("Dependency cycle detected: {0}")]
    DependencyCycle(String),

    #[error("Service not found: {0}")]
    ServiceNotFound(String),

    #[error("Infrastructure component not found: {0}")]
    InfraNotFound(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("State error: {0}")]
    StateError(String),

    #[error("Process error: {0}")]
    ProcessError(String),

    #[error("Tilt error: {0}")]
    TiltError(String),

    #[error("Plugin error: {0}")]
    PluginError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parsing error: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    #[error("JSON parsing error: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
