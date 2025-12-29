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
    #[default]
    Google,
    Aws,
    Azure,
}

impl Platform {
    pub fn as_str(&self) -> &'static str {
        match self {
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

/// Environment variables map.
pub type EnvVars = HashMap<String, String>;

/// Port number.
pub type Port = u16;

/// Instance ID (UUID or similar).
pub type InstanceId = String;

/// Service or component name.
pub type ComponentName = String;

