//! Profile types representing DevDock configuration.

use crate::types::{deserialize_env_vars, ComponentName, DeploymentType, EnvVars, Platform};
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
    #[serde(default, deserialize_with = "deserialize_env_vars")]
    pub env: EnvVars,

    /// Profile-level INI overrides (applied after all service-level overrides).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ini_overrides: Vec<IniOverride>,

    /// Multi-project config — additional project with its own services/infra.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi_project: Option<MultiProjectConfig>,

    /// DNS kubectl mode — "enabled" skips bastion dependency for remote service port-forwards.
    /// Set to "enabled" when the k8s cluster is directly accessible (no bastion tunnel needed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_kubectl: Option<String>,
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
    /// YAML booleans/numbers are coerced to strings (e.g. `DEBUG_MODE: True`).
    #[serde(default, deserialize_with = "deserialize_env_vars")]
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

/// Normalize the shorthand multi-project format in a raw YAML value.
///
/// If a top-level key is not in `KNOWN_PROFILE_KEYS` and its value contains
/// `infra` or `services` sub-keys, rewrite it as a canonical `multi_project:`
/// block with the key's name stored as `project`.
fn normalize_multi_project_key(value: &mut serde_yaml::Value) {
    let Some(map) = value.as_mapping_mut() else {
        return;
    };

    // Already has explicit multi_project — nothing to do.
    if map.contains_key("multi_project") {
        return;
    }

    // Find the first unknown key whose value looks like a project block.
    let extra_key = map
        .keys()
        .filter_map(|k| k.as_str())
        .filter(|k| !KNOWN_PROFILE_KEYS.contains(k))
        .find(|k| {
            map.get(*k).is_some_and(|v| v.get("infra").is_some() || v.get("services").is_some())
        })
        .map(String::from);

    let Some(extra_key) = extra_key else {
        return;
    };

    if let Some(mut extra_val) = map.remove(serde_yaml::Value::String(extra_key.clone())) {
        // Inject the project name into the multi-project block.
        if let Some(multi_map) = extra_val.as_mapping_mut() {
            multi_map.insert(
                serde_yaml::Value::String("project".to_string()),
                serde_yaml::Value::String(extra_key),
            );
        }
        map.insert(serde_yaml::Value::String("multi_project".to_string()), extra_val);
    }
}

/// Known top-level keys in a profile YAML — everything else is treated as a
/// shorthand multi-project block where the key itself is the project name.
const KNOWN_PROFILE_KEYS: &[&str] = &[
    "project",
    "platform",
    "region",
    "infra",
    "services",
    "pipelines",
    "test_suite",
    "env",
    "ini_overrides",
    "multi_project",
    "dns_kubectl",
];

impl Profile {
    /// Parse a profile from a YAML string.
    ///
    /// Supports both the canonical format with an explicit `multi_project:` key
    /// and the shorthand format where the secondary project name is used directly
    /// as a top-level key (e.g. `secondary-project: { infra: ..., services: ... }`).
    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        let mut value: serde_yaml::Value = serde_yaml::from_str(yaml)?;
        normalize_multi_project_key(&mut value);
        serde_yaml::from_value(value)
    }

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

    #[test]
    fn test_parse_profile_multi_project_shorthand() {
        // Shorthand format: secondary project name is the top-level YAML key.
        let yaml = r"
project: my-project
infra:
  sql:
    type: local
  opensearch:
    type: remote
services:
  qe:
    type: local
  fe:
    type: local
secondary-project:
  infra:
    sql:
      type: local
  services:
    central_datasources:
      type: local
      env:
        TEST_USER: user@example.com
";
        let profile = Profile::from_yaml(yaml).unwrap();

        // Top-level fields
        assert_eq!(profile.project, "my-project");
        assert!(profile.has_component("qe"));
        assert!(profile.has_component("fe"));
        assert!(profile.has_component("sql"));
        assert!(profile.has_component("opensearch"));

        // Multi-project parsed from shorthand key
        let multi = profile.multi_project.as_ref().expect("multi_project should be present");
        assert_eq!(multi.project, "secondary-project");
        assert!(multi.services.contains_key("central_datasources"));
        assert!(multi.infra.contains_key("sql"));

        // all_components includes secondary project services
        let all = profile.all_components();
        assert!(all.contains(&"central_datasources".to_string()));
    }
}
