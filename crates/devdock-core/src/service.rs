//! Service-related types and metadata.

use crate::types::{ComponentName, EnvVars, ServiceType};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Service metadata loaded from devdock_config.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceMetadata {
    /// Base Docker image.
    pub base_image: String,

    /// Bazel build target.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bazel_target: Option<String>,

    /// Binary mount path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_mount: Option<String>,

    /// Service type (go, python, java, etc.).
    #[serde(rename = "type")]
    pub service_type: ServiceType,

    /// Dependencies on other services/infra.
    #[serde(default)]
    pub dependencies: Vec<ComponentName>,

    /// Environment variables.
    #[serde(default)]
    pub env: EnvVars,

    /// Debug port for debugging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_port: Option<String>,

    /// Extra volume mounts.
    #[serde(default)]
    pub extra_mounts: EnvVars,

    /// Entrypoint configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<EntrypointConfig>,

    /// Healthcheck configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub healthcheck: Option<HealthcheckConfig>,
}

/// Entrypoint configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntrypointConfig {
    pub script: String,
}

/// Healthcheck configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthcheckConfig {
    pub test: Vec<String>,
}

impl ServiceMetadata {
    /// Load service metadata from devdock_config.json.
    pub fn load_from_path(path: &PathBuf) -> crate::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let metadata: ServiceMetadata = serde_json::from_str(&content)?;
        Ok(metadata)
    }
}

