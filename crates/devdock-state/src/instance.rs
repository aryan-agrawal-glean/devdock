//! DevDock instance representation.

use serde::{Deserialize, Serialize};

/// Origin of a DevDock session — distinguishes human-initiated from automated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionOrigin {
    Dev,
    Bot,
}

impl SessionOrigin {
    pub fn as_prefix(&self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Bot => "bot",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "bot" => Self::Bot,
            _ => Self::Dev,
        }
    }

    /// Derive origin from an existing devdock ID by inspecting its prefix.
    pub fn from_id(id: &str) -> Self {
        if id.starts_with("bot-") {
            Self::Bot
        } else {
            Self::Dev
        }
    }
}

/// A DevDock instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub workspace_path: String,
    pub workspace_name: Option<String>,
    pub branch: String,
    pub profile: String,
    pub mode: String,
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
    Quarantined,
}

impl InstanceStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Error => "error",
            Self::Quarantined => "quarantined",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "starting" => Self::Starting,
            "running" => Self::Running,
            "stopping" => Self::Stopping,
            "stopped" => Self::Stopped,
            "quarantined" => Self::Quarantined,
            _ => Self::Error,
        }
    }
}

impl Instance {
    /// Generate a new prefixed instance ID: `{origin}-{uuid_v4}`.
    pub fn generate_id(origin: SessionOrigin) -> String {
        format!("{}-{}", origin.as_prefix(), uuid::Uuid::new_v4())
    }

    /// Return a truncated ID suitable for display (prefix + first 8 hex chars).
    pub fn display_id(id: &str) -> &str {
        &id[..id.len().min(12)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_id_dev_has_correct_prefix() {
        let id = Instance::generate_id(SessionOrigin::Dev);
        assert!(id.starts_with("dev-"), "dev ID should start with 'dev-': {id}");
    }

    #[test]
    fn generate_id_bot_has_correct_prefix() {
        let id = Instance::generate_id(SessionOrigin::Bot);
        assert!(id.starts_with("bot-"), "bot ID should start with 'bot-': {id}");
    }

    #[test]
    fn generate_id_contains_valid_uuid() {
        let id = Instance::generate_id(SessionOrigin::Dev);
        let uuid_part = &id[4..]; // skip "dev-"
        assert!(
            uuid::Uuid::parse_str(uuid_part).is_ok(),
            "UUID portion should be valid: {uuid_part}"
        );
    }

    #[test]
    fn generate_id_is_unique() {
        let id1 = Instance::generate_id(SessionOrigin::Dev);
        let id2 = Instance::generate_id(SessionOrigin::Dev);
        assert_ne!(id1, id2);
    }

    #[test]
    fn generate_id_length() {
        // "dev-" (4) + UUID (36) = 40, "bot-" (4) + UUID (36) = 40
        let dev_id = Instance::generate_id(SessionOrigin::Dev);
        let bot_id = Instance::generate_id(SessionOrigin::Bot);
        assert_eq!(dev_id.len(), 40);
        assert_eq!(bot_id.len(), 40);
    }

    #[test]
    fn display_id_truncates_to_12() {
        let id = Instance::generate_id(SessionOrigin::Dev);
        let display = Instance::display_id(&id);
        assert_eq!(display.len(), 12);
        assert!(display.starts_with("dev-"));
    }

    #[test]
    fn display_id_safe_on_short_strings() {
        assert_eq!(Instance::display_id(""), "");
        assert_eq!(Instance::display_id("abc"), "abc");
        assert_eq!(Instance::display_id("dev-550e840"), "dev-550e840");
    }

    #[test]
    fn from_id_detects_dev() {
        let id = Instance::generate_id(SessionOrigin::Dev);
        assert_eq!(SessionOrigin::from_id(&id), SessionOrigin::Dev);
    }

    #[test]
    fn from_id_detects_bot() {
        let id = Instance::generate_id(SessionOrigin::Bot);
        assert_eq!(SessionOrigin::from_id(&id), SessionOrigin::Bot);
    }

    #[test]
    fn from_id_legacy_hex_defaults_to_dev() {
        // Old-format hex timestamp IDs should be treated as Dev
        assert_eq!(SessionOrigin::from_id("1945a3b2c1f"), SessionOrigin::Dev);
    }

    #[test]
    fn from_id_legacy_uuid_defaults_to_dev() {
        // Old-format bare UUIDs (no prefix) should be treated as Dev
        assert_eq!(
            SessionOrigin::from_id("550e8400-e29b-41d4-a716-446655440000"),
            SessionOrigin::Dev
        );
    }

    #[test]
    fn parse_roundtrips() {
        assert_eq!(SessionOrigin::parse("dev"), SessionOrigin::Dev);
        assert_eq!(SessionOrigin::parse("bot"), SessionOrigin::Bot);
        assert_eq!(SessionOrigin::parse("unknown"), SessionOrigin::Dev);
        assert_eq!(SessionOrigin::parse(""), SessionOrigin::Dev);
    }

    #[test]
    fn as_prefix_values() {
        assert_eq!(SessionOrigin::Dev.as_prefix(), "dev");
        assert_eq!(SessionOrigin::Bot.as_prefix(), "bot");
    }

    #[test]
    fn quarantined_status_roundtrips_and_is_distinct_from_error() {
        assert_eq!(InstanceStatus::parse("quarantined"), InstanceStatus::Quarantined);
        assert_eq!(InstanceStatus::Quarantined.as_str(), "quarantined");
        assert_eq!(InstanceStatus::parse("unknown-future-status"), InstanceStatus::Error);
    }
}
