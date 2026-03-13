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

    /// Profile-level INI overrides (applied after all service-level overrides).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ini_overrides: Vec<IniOverride>,

    /// Multi-project config — additional project with its own services/infra.
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

    /// INI overrides for this service.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ini_overrides: Vec<IniOverride>,

    /// Script path for pre_setup/post_setup hooks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,

    /// Target service for pre_setup/post_setup hooks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,

    /// Dependencies for hooks (services that must be ready first).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends_on_services: Option<Vec<String>>,

    /// Timeout for hooks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,

    /// Service-specific configuration.
    #[serde(flatten)]
    pub config: HashMap<String, serde_json::Value>,
}

/// INI override entry (can be at profile or service level).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IniOverride {
    pub section: String,
    pub key: String,
    pub val: String,
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
    #[serde(rename = "type", default = "default_test_type")]
    pub test_type: String,

    #[serde(flatten)]
    pub config: HashMap<String, serde_json::Value>,
}

fn default_test_type() -> String {
    "test".to_string()
}

/// Multi-project configuration — run services from an additional project
/// alongside the main project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiProjectConfig {
    /// The secondary project name (e.g., "my-other-project")
    pub project: String,

    #[serde(default)]
    pub infra: HashMap<ComponentName, InfraConfig>,

    #[serde(default)]
    pub services: HashMap<ComponentName, ServiceConfig>,
}

impl Profile {
    /// Get all component names from this profile.
    pub fn all_components(&self) -> Vec<ComponentName> {
        use std::collections::HashSet;

        let mut seen = HashSet::new();
        let mut components = Vec::new();

        // Add top-level components
        for name in self.infra.keys() {
            if seen.insert(name.clone()) {
                components.push(name.clone());
            }
        }
        for name in self.services.keys() {
            if seen.insert(name.clone()) {
                components.push(name.clone());
            }
        }
        for name in self.pipelines.keys() {
            if seen.insert(name.clone()) {
                components.push(name.clone());
            }
        }
        for name in self.test_suite.keys() {
            if seen.insert(name.clone()) {
                components.push(name.clone());
            }
        }

        // Include components from multi-project config if present (deduplicated)
        if let Some(ref multi) = self.multi_project {
            for name in multi.infra.keys() {
                if seen.insert(name.clone()) {
                    components.push(name.clone());
                }
            }
            for name in multi.services.keys() {
                if seen.insert(name.clone()) {
                    components.push(name.clone());
                }
            }
        }

        components
    }

    /// Check if a component exists in this profile.
    pub fn has_component(&self, name: &str) -> bool {
        self.infra.contains_key(name)
            || self.services.contains_key(name)
            || self.pipelines.contains_key(name)
            || self.test_suite.contains_key(name)
            || self
                .multi_project
                .as_ref()
                .is_some_and(|m| m.infra.contains_key(name) || m.services.contains_key(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_profile() {
        let yaml = r"
project: test-project
infra:
  sql:
    type: local
services:
  qe:
    type: local
";
        let profile: Profile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(profile.project, "test-project");
        assert_eq!(profile.infra.len(), 1);
        assert_eq!(profile.services.len(), 1);
        assert!(profile.has_component("sql"));
        assert!(profile.has_component("qe"));
        assert_eq!(profile.ini_overrides.len(), 0);
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

    #[test]
    fn test_parse_profile_with_test_suite_no_type() {
        let yaml = r"
project: test-project
services:
  qe:
    type: local
test_suite:
  test_example:
    dependencies:
      - qe
    stable_wait_seconds: 10
    params:
      json_test_filter: test_filter
";
        let profile: Profile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(profile.test_suite.len(), 1);
        let test_config = profile.test_suite.get("test_example").unwrap();
        assert_eq!(test_config.test_type, "test");
        assert!(test_config.config.contains_key("dependencies"));
    }

    #[test]
    fn test_parse_profile_with_test_suite_with_type() {
        let yaml = r"
project: test-project
services:
  qe:
    type: local
test_suite:
  test_example:
    type: integration_test
    dependencies:
      - qe
";
        let profile: Profile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(profile.test_suite.len(), 1);
        let test_config = profile.test_suite.get("test_example").unwrap();
        assert_eq!(test_config.test_type, "integration_test");
    }

    #[test]
    fn test_parse_profile_with_ini_overrides() {
        let yaml = r"
project: test-project
services:
  qe:
    type: local
    ini_overrides:
      - section: queryapi
        key: baseURL
        val: https://example.com
ini_overrides:
  - section: manifest
    key: llm_key_sku
    val: LKS_APP_KEY_UNIVERSAL
";
        let profile: Profile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(profile.ini_overrides.len(), 1);
        assert_eq!(profile.ini_overrides[0].section, "manifest");

        let qe_config = profile.services.get("qe").unwrap();
        assert_eq!(qe_config.ini_overrides.len(), 1);
        assert_eq!(qe_config.ini_overrides[0].section, "queryapi");
    }

    #[test]
    fn test_parse_profile_with_pre_setup() {
        let yaml = r"
project: test-project
services:
  crawler:
    type: local
  pre_setup:
    type: local
    script: devdock/config/services/service_hooks/pre_scripts/disable_datasources.sh
    service: crawler
    depends_on_services:
      - qe
    timeout: 10m
";
        let profile: Profile = serde_yaml::from_str(yaml).unwrap();

        let pre_setup = profile.services.get("pre_setup").unwrap();
        assert_eq!(
            pre_setup.script,
            Some(
                "devdock/config/services/service_hooks/pre_scripts/disable_datasources.sh"
                    .to_string()
            )
        );
        assert_eq!(pre_setup.service, Some("crawler".to_string()));
        assert_eq!(pre_setup.depends_on_services.as_ref().unwrap().len(), 1);
        assert_eq!(pre_setup.timeout, Some("10m".to_string()));
    }

    #[test]
    fn test_parse_profile_with_multi_project() {
        let yaml = r"
project: test-project
infra:
  sql:
    type: local
services:
  qe:
    type: local
multi_project:
  project: secondary-project
  infra:
    sql:
      type: local
  services:
    internal_admin:
      type: remote
";
        let profile: Profile = serde_yaml::from_str(yaml).unwrap();

        // Verify top-level components
        assert!(profile.has_component("sql"));
        assert!(profile.has_component("qe"));

        // Verify multi-project components are included
        assert!(profile.has_component("internal_admin"));

        // Verify all_components includes both top-level and multi-project
        let all_comps = profile.all_components();
        assert!(all_comps.contains(&"qe".to_string()));
        assert!(all_comps.contains(&"internal_admin".to_string()));
        assert!(all_comps.contains(&"sql".to_string()));

        // Verify multi_project structure
        let multi = profile.multi_project.as_ref().unwrap();
        assert_eq!(multi.services.len(), 1);
        assert_eq!(multi.infra.len(), 1);
        assert!(multi.services.contains_key("internal_admin"));
    }
}
