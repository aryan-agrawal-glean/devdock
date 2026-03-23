//! Service configuration parsed from `devdock_config.json`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Component kind — service (default) or infra.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComponentKind {
    Service,
    Infra,
}

impl Default for ComponentKind {
    fn default() -> Self {
        Self::Service
    }
}

impl std::fmt::Display for ComponentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Service => write!(f, "service"),
            Self::Infra => write!(f, "infra"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevdockServiceConfig {
    /// Component kind: "service" (default) or "infra".
    /// Infra components without a docker-compose.yaml get one generated.
    #[serde(default)]
    pub kind: ComponentKind,
    #[serde(rename = "type", default = "default_service_type")]
    pub service_type: String,
    /// Container image for infra components (e.g., "mysql:8.0").
    #[serde(default)]
    pub image: Option<String>,
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
    /// Generic build command for non-Bazel services (e.g. `["npm", "run", "build"]`).
    /// Creates a `{name}-build` local_resource in Tilt.
    #[serde(default)]
    pub build_command: Option<Vec<String>>,
    /// Path to the artifact produced by `build_command` (e.g. `"dist/"` or
    /// `"target/release/my-app"`). Mounted into the Docker container.
    #[serde(default)]
    pub build_output: Option<String>,
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

    /// Returns true if this service uses a non-Bazel build command.
    pub fn uses_generic_build(&self) -> bool {
        self.build_command.is_some()
    }

    /// Returns true if this service has any local build step (Bazel or generic).
    pub fn has_build_target(&self) -> bool {
        self.bazel_target.is_some() || self.build_command.is_some()
    }

    /// Returns true if this component is of kind "infra".
    pub fn is_infra(&self) -> bool {
        self.kind == ComponentKind::Infra
    }

    /// Generate a docker-compose.yaml for an infra component that has an image
    /// but no existing compose file.
    pub fn generate_compose(&self, name: &str) -> Option<String> {
        let image = self.image.as_deref()?;
        let port = self.port.as_deref().unwrap_or("8080");

        let mut compose = format!(
            "services:\n  {name}:\n    image: {image}\n    container_name: devdock-{name}\n"
        );

        // Ports
        compose.push_str(&format!("    ports:\n      - \"{port}:{port}\"\n"));

        // Environment
        if !self.env.is_empty() {
            compose.push_str("    environment:\n");
            let mut keys: Vec<_> = self.env.keys().collect();
            keys.sort();
            for key in keys {
                let val = &self.env[key];
                compose.push_str(&format!("      {key}: \"{val}\"\n"));
            }
        }

        // Healthcheck
        if let Some(hc) = &self.healthcheck {
            if let Some(test) = hc.get("test") {
                if let Some(arr) = test.as_array() {
                    let parts: Vec<String> =
                        arr.iter().filter_map(|v| v.as_str().map(|s| format!("\"{s}\""))).collect();
                    compose.push_str("    healthcheck:\n");
                    compose.push_str(&format!("      test: [{}]\n", parts.join(", ")));
                    compose.push_str("      interval: 5s\n      timeout: 5s\n      retries: 10\n");
                }
            }
        }

        // Networks
        compose.push_str(&format!(
            "    networks:\n\
             \x20     devdock_network:\n\
             \x20       aliases:\n\
             \x20         - {name}.devdock.local\n\
             \x20     devdock_infra:\n\
             \x20       aliases:\n\
             \x20         - {name}.infra.local\n\
             \x20   labels:\n\
             \x20     devdock.service: \"{name}\"\n\
             \x20     devdock.type: \"infra\"\n\
             \n\
             networks:\n\
             \x20 devdock_network:\n\
             \x20   external: true\n\
             \x20 devdock_infra:\n\
             \x20   external: true\n"
        ));

        Some(compose)
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
    fn default_kind_is_service() {
        let c = DevdockServiceConfig::parse(r#"{"type":"python"}"#).unwrap();
        assert_eq!(c.kind, ComponentKind::Service);
        assert!(!c.is_infra());
    }

    #[test]
    fn parse_infra_kind() {
        let c = DevdockServiceConfig::parse(
            r#"{"kind":"infra","image":"mysql:8.0","port":"3306","env":{"MYSQL_ALLOW_EMPTY_PASSWORD":"yes"},"healthcheck":{"test":["CMD","mysqladmin","ping"]}}"#,
        )
        .unwrap();
        assert_eq!(c.kind, ComponentKind::Infra);
        assert!(c.is_infra());
        assert_eq!(c.image.as_deref(), Some("mysql:8.0"));
    }

    #[test]
    fn generate_compose_for_infra() {
        let c = DevdockServiceConfig::parse(r#"{"kind":"infra","image":"redis:7","port":"6379"}"#)
            .unwrap();
        let compose = c.generate_compose("redis").unwrap();
        assert!(compose.contains("image: redis:7"));
        assert!(compose.contains("6379:6379"));
        assert!(compose.contains("devdock-redis"));
    }

    #[test]
    fn generate_compose_returns_none_without_image() {
        let c = DevdockServiceConfig::parse(r#"{"kind":"infra","port":"3306"}"#).unwrap();
        assert!(c.generate_compose("sql").is_none());
    }

    #[test]
    fn parse_build_command() {
        let c = DevdockServiceConfig::parse(
            r#"{"type":"node","build_command":["npm","run","build"],"build_output":"dist/"}"#,
        )
        .unwrap();
        assert_eq!(c.build_command, Some(vec!["npm".into(), "run".into(), "build".into()]));
        assert_eq!(c.build_output, Some("dist/".into()));
        assert!(c.uses_generic_build());
        assert!(c.has_build_target());
        assert!(!c.uses_bazel());
    }

    #[test]
    fn has_build_target_bazel() {
        let c = DevdockServiceConfig::parse(
            r#"{"type":"java","bazel_target":"//services/api:deploy"}"#,
        )
        .unwrap();
        assert!(c.has_build_target());
        assert!(c.uses_bazel());
        assert!(!c.uses_generic_build());
    }

    #[test]
    fn has_build_target_false_when_neither() {
        let c = DevdockServiceConfig::parse(r#"{"type":"python"}"#).unwrap();
        assert!(!c.has_build_target());
        assert!(!c.uses_bazel());
        assert!(!c.uses_generic_build());
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
