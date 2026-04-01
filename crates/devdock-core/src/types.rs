//! Common types used throughout DevDock.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Component type: service, infrastructure, or pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComponentType {
    Service,
    Infra,
    Pipeline,
}

/// Deployment type: local or remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeploymentType {
    Local,
    Remote,
}

/// Cloud platform type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    /// No cloud platform (pure OSS / local-only)
    #[default]
    None,
    Google,
    Aws,
    Azure,
}

impl Platform {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Google => "google",
            Self::Aws => "aws",
            Self::Azure => "azure",
        }
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Service language/runtime type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceType {
    Go,
    Python,
    Java,
    TypeScript,
    Custom(String),
}

/// Environment variables map — values are always strings.
///
/// During YAML parsing, non-string scalars (booleans, integers) are coerced
/// to their string representation so that `DEBUG_MODE: True` is accepted.
pub type EnvVars = HashMap<String, String>;

/// Deserialize a `HashMap<String, String>` where YAML values may be booleans
/// or numbers (e.g. `DEBUG_MODE: True`).  Coerces all scalars to strings.
pub fn deserialize_env_vars<'de, D>(deserializer: D) -> Result<EnvVars, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::MapAccess;

    struct EnvVisitor;

    impl<'de> serde::de::Visitor<'de> for EnvVisitor {
        type Value = EnvVars;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a map of string env var names to scalar values")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<EnvVars, A::Error> {
            let mut result = HashMap::new();
            while let Some((key, value)) = map.next_entry::<String, serde_yaml::Value>()? {
                let s = match &value {
                    serde_yaml::Value::String(s) => s.clone(),
                    serde_yaml::Value::Bool(b) => b.to_string(),
                    serde_yaml::Value::Number(n) => n.to_string(),
                    serde_yaml::Value::Null => String::new(),
                    other => serde_yaml::to_string(other).unwrap_or_default().trim().to_string(),
                };
                result.insert(key, s);
            }
            Ok(result)
        }
    }

    deserializer.deserialize_map(EnvVisitor)
}

/// Port number.
pub type Port = u16;

/// Instance ID (UUID or similar).
pub type InstanceId = String;

/// Service or component name.
pub type ComponentName = String;
