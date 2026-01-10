//! Service configuration structures.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for a single service loaded from devdock_config.json
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ServiceConfig {
    #[serde(default)]
    pub base_image: String,

    #[serde(rename = "type", default)]
    pub service_type: ServiceType,

    #[serde(default)]
    pub dependencies: Vec<String>,

    #[serde(default)]
    pub env: HashMap<String, String>,

    pub debug_port: Option<String>,

    pub bazel_target: Option<String>,

    pub binary_mount: Option<String>,

    pub binary: Option<String>,

    pub target_is_directory: Option<String>,

    #[serde(default)]
    pub extra_mounts: HashMap<String, String>,

    pub entrypoint: Option<Entrypoint>,

    pub healthcheck: Option<Healthcheck>,

    #[serde(default)]
    pub ini_overrides: Vec<IniOverride>,

    /// Deployment type from profile (local/remote) - set after loading
    #[serde(skip)]
    pub deployment_local: bool,
}

/// Service type (language/runtime from devdock_config.json)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ServiceType {
    Go,
    Java,
    Python,
    #[default]
    Remote,
    Pipeline,
    #[serde(rename = "test_suite")]
    TestSuite,
    /// For services with minimal config (like fe with only dependencies)
    #[serde(rename = "minimal")]
    Minimal,
}

/// Entrypoint configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Entrypoint {
    pub script: String,
}

/// Healthcheck configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Healthcheck {
    pub test: Vec<String>,
}

/// INI override entry
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IniOverride {
    pub section: String,
    pub key: String,
    pub val: String,
}

impl ServiceConfig {
    /// Check if this is a local service (based on profile's deployment type)
    pub fn is_local(&self) -> bool {
        self.deployment_local
    }

    /// Set deployment type from profile
    pub fn set_deployment_local(&mut self, is_local: bool) {
        self.deployment_local = is_local;
    }

    /// Get the target directory flag
    pub fn is_target_directory(&self) -> bool {
        self.target_is_directory.as_ref().map_or(true, |s| s.to_lowercase() == "true")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_qe_config() {
        let json = r#"{
            "base_image": "debian:bookworm-slim",
            "bazel_target": "//go/platform/gcp/query_endpoint:query_endpoint_binary",
            "binary_mount": "go/platform/gcp/query_endpoint",
            "type": "go",
            "dependencies": ["sql", "pubsub"],
            "env": {
                "PORT": "7960"
            },
            "debug_port": "2345"
        }"#;

        let config: ServiceConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.service_type, ServiceType::Go);
        assert_eq!(config.dependencies.len(), 2);
        assert_eq!(config.env.get("PORT"), Some(&"7960".to_string()));
    }
}
