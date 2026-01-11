//! DevDock instance representation.

use serde::{Deserialize, Serialize};

/// A DevDock instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub workspace_path: String,
    pub workspace_name: Option<String>, // For remote instances: coder workspace name
    pub branch: String,
    pub profile: String,
    pub mode: String, // "local" or "remote"
    pub status: InstanceStatus,
    pub port_base: u16,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Instance status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstanceStatus {
    Starting,
    Running,
    Stopping,
    Stopped,
    Error,
}

impl InstanceStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "starting" => Self::Starting,
            "running" => Self::Running,
            "stopping" => Self::Stopping,
            "stopped" => Self::Stopped,
            _ => Self::Error,
        }
    }
}

impl Instance {
    /// Generate a new instance ID.
    pub fn generate_id() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
        format!("{timestamp:x}")
    }
}
