//! Typed context structs passed to plugins at each lifecycle transition.

use crate::hooks::ResourceKind;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Context after profile is loaded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileContext {
    pub profile_name: String,
    pub workspace_root: String,
    pub platform: String,
    pub project_name: String,
}

/// Context for infrastructure setup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfraContext {
    pub profile_name: String,
    pub workspace_root: String,
    pub platform: String,
    pub instance_id: String,
    pub port_base: u16,
}

/// Context for config generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigContext {
    pub profile_name: String,
    pub workspace_root: String,
    pub runtime_dir: String,
    pub platform: String,
    pub instance_id: String,
}

/// Context for Tilt operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TiltContext {
    pub profile_name: String,
    pub session_id: String,
    pub workspace_root: String,
}

/// Context for per-resource hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceContext {
    pub resource_name: String,
    pub resource_kind: ResourceKind,
    pub session_id: String,
    pub workspace_root: String,
    pub config: serde_json::Value,
}

/// Context when all resources are healthy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadyContext {
    pub session_id: String,
    pub profile_name: String,
    pub resource_count: usize,
    pub startup_duration_secs: f64,
}

/// Context for shutdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopContext {
    pub session_id: String,
    pub duration_secs: i64,
}

/// Context for errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorContext {
    pub session_id: String,
    pub error: String,
    pub state: String,
    pub details: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_context_serializes() {
        let ctx = ProfileContext {
            profile_name: "default".into(),
            workspace_root: "/workspace/repo".into(),
            platform: "none".into(),
            project_name: "test-app".into(),
        };
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["profile_name"], "default");
        assert_eq!(json["project_name"], "test-app");
    }

    #[test]
    fn resource_context_serializes() {
        let ctx = ResourceContext {
            resource_name: "sql".into(),
            resource_kind: ResourceKind::Infra,
            session_id: "dev-123".into(),
            workspace_root: "/workspace".into(),
            config: serde_json::json!({}),
        };
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["resource_name"], "sql");
        assert_eq!(json["resource_kind"], "infra");
    }
}
