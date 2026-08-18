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
                | ResourceLabel::BazelBuilds // Now included for telemetry
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

/// One Tilt build-history entry. Its identity is meaningful only with the
/// enclosing snapshot's `tiltStartTime` and resource name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TiltBuild {
    pub span_id: String,
    pub start_time: String,
    pub finish_time: Option<String>,
    pub error: Option<String>,
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
    pub build_start_time: Option<String>,
    pub build_finish_time: Option<String>,
    pub build_duration_secs: Option<f64>,
    pub build_history: Vec<TiltBuild>,
}

impl TiltResource {
    /// Create a new TiltResource from raw Tilt JSON data.
    pub fn from_json(item: &serde_json::Value) -> Option<Self> {
        let metadata = item.get("metadata")?;
        let status = item.get("status")?;

        let name = metadata.get("name")?.as_str()?.to_string();

        // Labels can be either an object {"service":"service"} or an array ["service"]
        let labels: Vec<ResourceLabel> = metadata
            .get("labels")
            .map(|l| {
                if let Some(obj) = l.as_object() {
                    // Object format: keys are the label names
                    obj.keys().map(|k| ResourceLabel::from(k.as_str())).collect()
                } else if let Some(arr) = l.as_array() {
                    // Array format: values are the label names
                    arr.iter().filter_map(|v| v.as_str()).map(ResourceLabel::from).collect()
                } else {
                    Vec::new()
                }
            })
            .unwrap_or_default();

        let resource_type = Self::determine_resource_type(&labels);

        let is_disabled =
            status.get("disableStatus").and_then(|d| d.get("state")).and_then(|s| s.as_str())
                == Some("Disabled");

        let runtime_status = status.get("runtimeStatus").and_then(|s| s.as_str()).map(String::from);

        let update_status = status.get("updateStatus").and_then(|s| s.as_str()).map(String::from);

        let compose_health = status
            .get("composeResourceInfo")
            .and_then(|c| c.get("healthStatus"))
            .and_then(|h| h.as_str())
            .map(String::from);

        let resource_status =
            Self::determine_status(&runtime_status, &update_status, &compose_health, status);

        // Extract build timing from buildHistory
        let build_history = Self::extract_build_history(status);
        let (build_start, build_finish, build_duration) =
            Self::extract_build_timing(&build_history);

        Some(TiltResource {
            name,
            labels,
            resource_type,
            status: resource_status,
            runtime_status,
            update_status,
            compose_health,
            is_disabled,
            build_start_time: build_start,
            build_finish_time: build_finish,
            build_duration_secs: build_duration,
            build_history,
        })
    }

    fn extract_build_history(status: &serde_json::Value) -> Vec<TiltBuild> {
        status
            .get("buildHistory")
            .and_then(|history| history.as_array())
            .into_iter()
            .flatten()
            .filter_map(|build| {
                let span_id = build.get("spanID")?.as_str()?.to_string();
                let start_time = build.get("startTime")?.as_str()?.to_string();
                let finish_time =
                    build.get("finishTime").and_then(|value| value.as_str()).map(str::to_string);
                let error = build
                    .get("error")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
                Some(TiltBuild { span_id, start_time, finish_time, error })
            })
            .collect()
    }

    /// Extract build timing from buildHistory (most recent build)
    fn extract_build_timing(
        build_history: &[TiltBuild],
    ) -> (Option<String>, Option<String>, Option<f64>) {
        let latest_build = match build_history.first() {
            Some(b) => b,
            None => return (None, None, None),
        };
        let start_time = Some(latest_build.start_time.clone());
        let finish_time = latest_build.finish_time.clone();

        // Calculate duration if both times exist
        let duration = if let (Some(start), Some(finish)) = (&start_time, &finish_time) {
            // Parse ISO8601 timestamps and calculate difference
            use chrono::{DateTime, Utc};
            match (DateTime::parse_from_rfc3339(start), DateTime::parse_from_rfc3339(finish)) {
                (Ok(start_dt), Ok(finish_dt)) => {
                    let start_utc = start_dt.with_timezone(&Utc);
                    let finish_utc = finish_dt.with_timezone(&Utc);
                    let duration = finish_utc.signed_duration_since(start_utc);
                    Some(duration.num_milliseconds() as f64 / 1000.0)
                }
                _ => None,
            }
        } else {
            None
        };

        (start_time, finish_time, duration)
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

        let has_error =
            status_obj.get("error").and_then(|e| e.as_str()).is_some_and(|s| !s.is_empty());
        if has_error {
            return ResourceStatus::Failed;
        }

        let ready_condition = status_obj
            .get("conditions")
            .and_then(|c| c.as_array())
            .and_then(|arr| {
                arr.iter().find(|cond| cond.get("type").and_then(|t| t.as_str()) == Some("Ready"))
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
                } else if runtime_status.as_deref() == Some("ok") && ready_condition == Some("True")
                {
                    return ResourceStatus::Healthy;
                }
            }
            Some("local") => {
                if matches!(runtime_status.as_deref(), Some("ok" | "not_applicable"))
                    && ready_condition == Some("True")
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
        !self.is_disabled && self.labels.iter().any(ResourceLabel::is_monitored)
    }
}
