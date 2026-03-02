//! Service configuration parsed from `devdock_config.json`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevdockServiceConfig {
    #[serde(rename = "type", default = "default_service_type")]
    pub service_type: String,
    #[serde(default)]
    pub port: Option<String>,
    #[serde(default)]
    pub debug_port: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_env_map")]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub bazel_target: Option<String>,
    #[serde(default)]
    pub binary_mount: Option<String>,
    #[serde(default)]
    pub base_image: Option<String>,
    #[serde(default)]
    pub healthcheck: Option<serde_json::Value>,
    #[serde(default)]
    pub watch_patterns: Vec<String>,
    #[serde(default)]
    pub extra_mounts: HashMap<String, String>,
    #[serde(default)]
    pub entrypoint: Option<Entrypoint>,
    // Accept unknown fields gracefully
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entrypoint {
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub command: Option<Vec<String>>,
}

fn default_service_type() -> String {
    "custom".into()
}

/// Accept mixed types in env (string, bool, number → string).
fn deserialize_env_map<'de, D>(
    deserializer: D,
) -> std::result::Result<HashMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: HashMap<String, serde_json::Value> = HashMap::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .map(|(k, v)| {
            let s = match &v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Number(n) => n.to_string(),
                other => other.to_string(),
            };
            (k, s)
        })
        .collect())
}

impl DevdockServiceConfig {
    pub fn load(path: &Path) -> crate::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::Error::Config(format!("Failed to read {}: {e}", path.display())))?;
        Self::parse(&content)
    }

    pub fn parse(content: &str) -> crate::Result<Self> {
        serde_json::from_str(content)
            .map_err(|e| crate::Error::Config(format!("Invalid devdock_config.json: {e}")))
    }

    pub fn uses_bazel(&self) -> bool {
        self.bazel_target.is_some()
    }
}

/// Discover all service configs in a directory.
pub fn discover_services(
    services_dir: &Path,
) -> crate::Result<HashMap<String, DevdockServiceConfig>> {
    let mut configs = HashMap::new();
    if !services_dir.exists() {
        return Ok(configs);
    }

    let entries = std::fs::read_dir(services_dir).map_err(|e| {
        crate::Error::Config(format!("Failed to read {}: {e}", services_dir.display()))
    })?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let config_path = path.join("devdock_config.json");
            if config_path.exists() {
                let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                let config = DevdockServiceConfig::load(&config_path)?;
                configs.insert(name, config);
            }
        }
    }
    Ok(configs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal() {
        let c = DevdockServiceConfig::parse(r#"{"type":"python"}"#).unwrap();
        assert_eq!(c.service_type, "python");
        assert!(c.dependencies.is_empty());
    }

    #[test]
    fn missing_type_defaults_custom() {
        let c = DevdockServiceConfig::parse(r#"{"port":"8080"}"#).unwrap();
        assert_eq!(c.service_type, "custom");
    }

    #[test]
    fn parse_with_deps() {
        let c =
            DevdockServiceConfig::parse(r#"{"type":"go","dependencies":["sql","redis"]}"#).unwrap();
        assert_eq!(c.dependencies, vec!["sql", "redis"]);
    }

    #[test]
    fn bool_env_converted() {
        let c = DevdockServiceConfig::parse(r#"{"type":"python","env":{"DEBUG":true}}"#).unwrap();
        assert_eq!(c.env.get("DEBUG"), Some(&"true".to_string()));
    }

    #[test]
    fn unknown_fields_accepted() {
        let c = DevdockServiceConfig::parse(
            r#"{"type":"python","copy_runfiles":true,"target_is_directory":"True"}"#,
        )
        .unwrap();
        assert_eq!(c.service_type, "python");
    }

    #[test]
    fn discover_from_fixture() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("tests/fixtures/multi-service/devdock/config/services");
        if fixture.exists() {
            let configs = discover_services(&fixture).unwrap();
            assert_eq!(configs.len(), 3);
            assert!(configs.contains_key("api"));
            assert!(configs.contains_key("worker"));
        }
    }
}
