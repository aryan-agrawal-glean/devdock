//! Service-related types and metadata.

use crate::types::{ComponentName, EnvVars, ServiceType};
use crate::update::ServiceIdentity;
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

    /// Optional generation identities. Old service metadata omits this field;
    /// consumers must treat each missing value as unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<ServiceIdentity>,

    /// Optional status emitted by a later transaction-aware integration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ServiceStatusMetadata>,

    /// Optional redeploy/reload contract for packaged or live-mounted config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redeploy: Option<RedeployMetadata>,
}

/// Compatibility metadata for expected/built/staged/deployed/loaded identity.
/// Every field remains optional so old `devdock_config.json` files deserialize
/// unchanged without inventing an identity.
pub type ServiceIdentityMetadata = ServiceIdentity;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceStatusMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RedeployMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reload: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ServiceType;

    #[test]
    fn old_service_metadata_deserializes_with_unknown_optional_identity() {
        let metadata: ServiceMetadata = serde_json::from_str(
            r#"{
                "base_image": "devdock/test",
                "type": "python"
            }"#,
        )
        .unwrap();

        assert_eq!(metadata.service_type, ServiceType::Python);
        assert!(metadata.identity.is_none());
        assert!(metadata.status.is_none());
        assert!(metadata.redeploy.is_none());
    }

    #[test]
    fn new_identity_and_redeploy_metadata_roundtrip_without_secrets() {
        let metadata: ServiceMetadata = serde_json::from_str(
            r#"{
                "base_image": "devdock/test",
                "type": "python",
                "identity": {
                    "expected": "source:2",
                    "built": "build:2",
                    "staged": "stage:2",
                    "deployed": "deploy:2",
                    "loaded": "load:2"
                },
                "status": {"generation": "generation-2", "stage": "loaded_identity", "ready": true},
                "redeploy": {"strategy": "live_mount", "reload": "restart", "generation": "generation-2"}
            }"#,
        )
        .unwrap();

        assert_eq!(metadata.identity.as_ref().unwrap().loaded.as_deref(), Some("load:2"));
        assert_eq!(metadata.status.as_ref().unwrap().ready, Some(true));
        assert_eq!(metadata.redeploy.as_ref().unwrap().strategy.as_deref(), Some("live_mount"));
    }
}
