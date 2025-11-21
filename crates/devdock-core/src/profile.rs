//! Profile types representing DevDock configuration.

use crate::types::{ComponentName, DeploymentType, EnvVars, Platform};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A DevDock profile - the main configuration unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    /// GCP/AWS/Azure project ID.
    pub project: String,

    /// Cloud platform (google, aws, azure).
    #[serde(default)]
    pub platform: Platform,

    /// Infrastructure components (sql, opensearch, redis, etc.).
    #[serde(default)]
    pub infra: HashMap<ComponentName, InfraConfig>,

    /// Services (qe, fe, crawler, etc.).
    #[serde(default)]
    pub services: HashMap<ComponentName, ServiceConfig>,

    /// Pipelines (optional).
    #[serde(default)]
    pub pipelines: HashMap<ComponentName, PipelineConfig>,

    /// Test suites (optional).
    #[serde(default)]
    pub test_suite: HashMap<ComponentName, TestSuiteConfig>,

    /// Global environment variables.
    #[serde(default)]
    pub env: EnvVars,

    /// Multi-project config (e.g., glean-apps-canary).
    #[serde(rename = "glean-apps-canary", default, skip_serializing_if = "Option::is_none")]
    pub multi_project: Option<MultiProjectConfig>,
}

/// Infrastructure component configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfraConfig {
    /// Deployment type (local or remote).
    #[serde(rename = "type")]
    pub deployment_type: DeploymentType,

    /// Component-specific configuration.
    #[serde(flatten)]
    pub config: HashMap<String, serde_json::Value>,
}

/// Service configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// Deployment type (local or remote).
    #[serde(rename = "type")]
    pub deployment_type: DeploymentType,

    /// Environment variables for this service.
    #[serde(default)]
    pub env: EnvVars,

    /// Hot reload setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hot_reload: Option<HotReloadConfig>,

    /// Service-specific configuration.
    #[serde(flatten)]
    pub config: HashMap<String, serde_json::Value>,
}

/// Hot reload configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HotReloadConfig {
    Enabled(bool),
    Setting(String), // "enable" or "disable"
}

/// Pipeline configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    #[serde(rename = "type")]
    pub pipeline_type: String,

    #[serde(flatten)]
    pub config: HashMap<String, serde_json::Value>,
}

/// Test suite configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSuiteConfig {
    #[serde(rename = "type")]
    pub test_type: String,

    #[serde(flatten)]
    pub config: HashMap<String, serde_json::Value>,
}

/// Multi-project configuration (for glean-apps-canary pattern).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiProjectConfig {
    #[serde(default)]
    pub infra: HashMap<ComponentName, InfraConfig>,

    #[serde(default)]
    pub services: HashMap<ComponentName, ServiceConfig>,
}

impl Profile {
    /// Get all component names from this profile.
    pub fn all_components(&self) -> Vec<ComponentName> {
        let mut components = Vec::new();
        components.extend(self.infra.keys().cloned());
        components.extend(self.services.keys().cloned());
        components.extend(self.pipelines.keys().cloned());
        components.extend(self.test_suite.keys().cloned());
        components
    }

    /// Check if a component exists in this profile.
    pub fn has_component(&self, name: &str) -> bool {
        self.infra.contains_key(name)
            || self.services.contains_key(name)
            || self.pipelines.contains_key(name)
            || self.test_suite.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_profile() {
        let yaml = r#"
project: dev-sandbox-334901
infra:
  sql:
    type: local
services:
  qe:
    type: local
"#;
        let profile: Profile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(profile.project, "dev-sandbox-334901");
        assert_eq!(profile.infra.len(), 1);
        assert_eq!(profile.services.len(), 1);
        assert!(profile.has_component("sql"));
        assert!(profile.has_component("qe"));
    }

    #[test]
    fn test_parse_profile_with_env() {
        let yaml = r#"
project: test-project
services:
  qe:
    type: local
    env:
      DEBUG_MODE: "true"
      PORT: "7960"
"#;
        let profile: Profile = serde_yaml::from_str(yaml).unwrap();
        let qe_config = profile.services.get("qe").unwrap();
        assert_eq!(qe_config.env.get("DEBUG_MODE").unwrap(), "true");
        assert_eq!(qe_config.env.get("PORT").unwrap(), "7960");
    }
}

