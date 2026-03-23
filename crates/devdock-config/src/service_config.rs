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

    #[serde(default, deserialize_with = "deserialize_env_map")]
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

/// Custom deserializer for env maps that handles both strings and other JSON types
fn deserialize_env_map<'de, D>(deserializer: D) -> Result<HashMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    use serde_json::Value;

    let value: HashMap<String, Value> = HashMap::deserialize(deserializer)?;
    let mut result = HashMap::new();

    for (key, val) in value {
        let string_val = match val {
            Value::String(s) => s,
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::Null => String::new(),
            _ => {
                return Err(D::Error::custom(format!(
                    "Env value for key '{}' must be a string, boolean, or number",
                    key
                )))
            }
        };
        result.insert(key, string_val);
    }

    Ok(result)
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

    #[test]
    fn test_parse_config_with_script_entrypoint() {
        let json = r#"{
            "base_image": "test:latest",
            "type": "java",
            "entrypoint": {"script": "entrypoint.sh"},
            "env": {}
        }"#;

        let config: ServiceConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.entrypoint.unwrap().script, "entrypoint.sh");
    }

    #[test]
    fn test_parse_config_with_array_entrypoint_fails() {
        let json = r#"{
            "base_image": "test:latest",
            "type": "java",
            "entrypoint": ["java", "-jar", "/app/foo.jar"],
            "env": {}
        }"#;

        let result = serde_json::from_str::<ServiceConfig>(json);
        assert!(result.is_err(), "Array-style entrypoint should fail parsing");
    }

    #[test]
    fn test_parse_config_without_entrypoint() {
        let json = r#"{
            "base_image": "test:latest",
            "type": "java",
            "env": {}
        }"#;

        let config: ServiceConfig = serde_json::from_str(json).unwrap();
        assert!(config.entrypoint.is_none());
    }

    #[test]
    fn test_parse_config_with_boolean_env() {
        let json = r#"{
            "base_image": "test:latest",
            "type": "java",
            "env": {
                "STRING_VAL": "hello",
                "BOOL_VAL": true,
                "NUMBER_VAL": 42,
                "PORT": "8080"
            }
        }"#;

        let config: ServiceConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.env.get("STRING_VAL"), Some(&"hello".to_string()));
        assert_eq!(config.env.get("BOOL_VAL"), Some(&"true".to_string()));
        assert_eq!(config.env.get("NUMBER_VAL"), Some(&"42".to_string()));
        assert_eq!(config.env.get("PORT"), Some(&"8080".to_string()));
    }
}
