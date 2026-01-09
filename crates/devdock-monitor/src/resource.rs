//! Resource types and status models for Tilt resources.

use serde::{Deserialize, Serialize};

/// Labels that identify resource types in Tilt.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceLabel {
    Service,
    Infra,
    RemoteService,
    RemoteInfra,
    BazelBuilds,
    #[serde(other)]
    Unknown,
}

impl ResourceLabel {
    /// Returns true if this label should be monitored during startup.
    pub fn is_monitored(&self) -> bool {
        matches!(
            self,
            ResourceLabel::Service
                | ResourceLabel::Infra
                | ResourceLabel::RemoteService
                | ResourceLabel::RemoteInfra
        )
    }
}

impl From<&str> for ResourceLabel {
    fn from(s: &str) -> Self {
        match s {
            "service" => ResourceLabel::Service,
            "infra" => ResourceLabel::Infra,
            "remote-service" => ResourceLabel::RemoteService,
            "remote-infra" => ResourceLabel::RemoteInfra,
            "bazel-builds" => ResourceLabel::BazelBuilds,
            _ => ResourceLabel::Unknown,
        }
    }
}

/// Type of resource based on its labels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceType {
    Local,
    Remote,
    BazelBuild,
    Unknown,
}

/// Health status of a Tilt resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceStatus {
    Healthy,
    Pending,
    Failed,
    Disabled,
    Unknown,
}

impl ResourceStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, ResourceStatus::Healthy | ResourceStatus::Failed)
    }

    pub fn is_healthy(&self) -> bool {
        matches!(self, ResourceStatus::Healthy)
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, ResourceStatus::Failed)
    }
}

/// A Tilt UI resource with its current status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TiltResource {
    pub name: String,
    pub labels: Vec<ResourceLabel>,
    pub resource_type: ResourceType,
    pub status: ResourceStatus,
    pub runtime_status: Option<String>,
    pub update_status: Option<String>,
    pub compose_health: Option<String>,
    pub is_disabled: bool,
}

impl TiltResource {
    /// Create a new TiltResource from raw Tilt JSON data.
    pub fn from_json(item: &serde_json::Value) -> Option<Self> {
        let metadata = item.get("metadata")?;
        let status = item.get("status")?;

        let name = metadata.get("name")?.as_str()?.to_string();

        let labels: Vec<ResourceLabel> = metadata
            .get("labels")
            .and_then(|l| l.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(ResourceLabel::from)
                    .collect()
            })
            .unwrap_or_default();

        let resource_type = Self::determine_resource_type(&labels);

        let is_disabled = status
            .get("disableStatus")
            .and_then(|d| d.get("state"))
            .and_then(|s| s.as_str())
            == Some("Disabled");

        let runtime_status = status
            .get("runtimeStatus")
            .and_then(|s| s.as_str())
            .map(String::from);

        let update_status = status
            .get("updateStatus")
            .and_then(|s| s.as_str())
            .map(String::from);

        let compose_health = status
            .get("composeResourceInfo")
            .and_then(|c| c.get("healthStatus"))
            .and_then(|h| h.as_str())
            .map(String::from);

        let resource_status = Self::determine_status(
            &runtime_status,
            &update_status,
            &compose_health,
            status,
        );

        Some(TiltResource {
            name,
            labels,
            resource_type,
            status: resource_status,
            runtime_status,
            update_status,
            compose_health,
            is_disabled,
        })
    }

    fn determine_resource_type(labels: &[ResourceLabel]) -> ResourceType {
        for label in labels {
            match label {
                ResourceLabel::RemoteService | ResourceLabel::RemoteInfra => {
                    return ResourceType::Remote
                }
                ResourceLabel::Service | ResourceLabel::Infra => return ResourceType::Local,
                ResourceLabel::BazelBuilds => return ResourceType::BazelBuild,
                _ => {}
            }
        }
        ResourceType::Unknown
    }

    fn determine_status(
        runtime_status: &Option<String>,
        update_status: &Option<String>,
        compose_health: &Option<String>,
        status_obj: &serde_json::Value,
    ) -> ResourceStatus {
        if let Some(health) = compose_health {
            if health == "unhealthy" {
                return ResourceStatus::Failed;
            }
        }

        if let Some(us) = update_status {
            if us == "error" {
                return ResourceStatus::Failed;
            }
        }

        let has_error = status_obj
            .get("error")
            .and_then(|e| e.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        if has_error {
            return ResourceStatus::Failed;
        }

        let ready_condition = status_obj
            .get("conditions")
            .and_then(|c| c.as_array())
            .and_then(|arr| {
                arr.iter().find(|cond| {
                    cond.get("type").and_then(|t| t.as_str()) == Some("Ready")
                })
            })
            .and_then(|cond| cond.get("status"))
            .and_then(|s| s.as_str());

        let spec_type = status_obj
            .get("specs")
            .and_then(|s| s.as_array())
            .and_then(|arr| arr.first())
            .and_then(|spec| spec.get("type"))
            .and_then(|t| t.as_str());

        match spec_type {
            Some("docker-compose") => {
                if let Some(health) = compose_health {
                    if health == "unhealthy" {
                        return ResourceStatus::Failed;
                    }
                    if runtime_status.as_deref() == Some("ok")
                        && ready_condition == Some("True")
                        && (compose_health.is_none() || health == "healthy")
                    {
                        return ResourceStatus::Healthy;
                    }
                } else if runtime_status.as_deref() == Some("ok")
                    && ready_condition == Some("True")
                {
                    return ResourceStatus::Healthy;
                }
            }
            Some("local") => {
                if matches!(
                    runtime_status.as_deref(),
                    Some("ok") | Some("not_applicable")
                ) && ready_condition == Some("True")
                {
                    return ResourceStatus::Healthy;
                }
            }
            _ => {
                if runtime_status.as_deref() == Some("ok") && ready_condition == Some("True") {
                    return ResourceStatus::Healthy;
                }
            }
        }

        ResourceStatus::Pending
    }

    /// Returns true if this resource should be monitored during startup.
    pub fn should_monitor(&self) -> bool {
        !self.is_disabled && self.labels.iter().any(|l| l.is_monitored())
    }
}

