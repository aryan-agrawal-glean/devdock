//! Data models for telemetry metrics.
//!
//! These models match the BigQuery table schemas for compatibility
//! with existing analytics infrastructure.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Session-level metrics for a DevDock session.
/// Table: devdock.devdock_session_stats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetrics {
    pub devdock_id: String,
    pub machine_id: String,
    pub architecture: String,
    pub platform: String,
    pub project: String,
    pub profile: String,
    pub branch: Option<String>,
    pub command_start_time: DateTime<Utc>,
    pub tilt_up_time: Option<DateTime<Utc>>,
    pub watch_start_time: Option<DateTime<Utc>>,
    pub startup_complete_time: Option<DateTime<Utc>>,
    pub tilt_bootstrap_seconds: Option<f64>,
    pub total_startup_seconds: Option<f64>,
    pub startup_status: String,
    pub total_resources: i32,
    pub healthy_resources: i32,
    pub failed_resources: i32,
    pub timestamp: DateTime<Utc>,
}

/// Per-resource status snapshot.
/// Table: devdock.resource_stats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceStatusMetrics {
    pub devdock_id: String,
    pub machine_id: String,
    pub timestamp: DateTime<Utc>,
    pub resource_name: String,
    #[serde(rename = "type")]
    pub resource_type: String,
    pub status: String,
}

/// Per-resource startup metrics.
/// Table: devdock.startup_stats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceStartupMetrics {
    pub devdock_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub resource_type: String,
    pub bootup_time: Option<f64>,
    pub time_to_first_healthy: Option<f64>,
    pub time_to_stable: Option<f64>,
    pub state: String,
    pub build_id: Option<String>,
    pub load_order: i32,
    pub creation_time: Option<DateTime<Utc>>,
    pub restart_count: i32,
    pub error_count: i32,
}

/// Host system metrics.
/// Table: devdock.host_stats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostMetrics {
    pub devdock_id: String,
    pub machine_id: String,
    pub platform: String,
    pub architecture: String,
    pub project: String,
    pub profile: String,
    pub cpu_percent: f64,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub memory_percent: f64,
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub disk_percent: f64,
    pub load_average_1m: Option<f64>,
    pub load_average_5m: Option<f64>,
    pub load_average_15m: Option<f64>,
    pub timestamp: DateTime<Utc>,
}

/// Docker container metrics.
/// Table: devdock.docker_stats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerMetrics {
    pub devdock_id: String,
    pub machine_id: String,
    pub container_id: String,
    pub name: String,
    pub cpu_percent: f64,
    pub memory_usage_bytes: u64,
    pub memory_limit_bytes: u64,
    pub memory_percent: f64,
    pub network_input_bytes: u64,
    pub network_output_bytes: u64,
    pub block_input_bytes: u64,
    pub block_output_bytes: u64,
    pub pids: u32,
    pub timestamp: DateTime<Utc>,
}

/// Resource failure log entry.
/// Table: devdock.resource_failure_logs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureLogMetrics {
    pub devdock_id: String,
    pub machine_id: String,
    pub timestamp: DateTime<Utc>,
    pub resource_name: String,
    #[serde(rename = "type")]
    pub resource_type: String,
    pub unhealthy_count: i32,
    pub log_gcs_url: Option<String>,
    pub error_message: Option<String>,
    pub processed: bool,
    pub processed_at: Option<DateTime<Utc>>,
}

/// Build metrics for bazel builds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildMetrics {
    pub devdock_id: String,
    pub resource_name: String,
    pub build_id: String,
    pub start_time: DateTime<Utc>,
    pub finish_time: Option<DateTime<Utc>>,
    pub duration_seconds: Option<f64>,
    pub status: String,
    pub error_message: Option<String>,
}

/// Resource health transition event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthTransitionEvent {
    pub devdock_id: String,
    pub resource_name: String,
    pub from_status: String,
    pub to_status: String,
    pub timestamp: DateTime<Utc>,
    pub duration_in_previous_state_seconds: Option<f64>,
}

/// Aggregated telemetry snapshot containing all metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TelemetrySnapshot {
    pub session: Option<SessionMetrics>,
    pub resource_statuses: Vec<ResourceStatusMetrics>,
    pub resource_startups: Vec<ResourceStartupMetrics>,
    pub host: Option<HostMetrics>,
    pub docker: Vec<DockerMetrics>,
    pub failures: Vec<FailureLogMetrics>,
    pub builds: Vec<BuildMetrics>,
    pub health_transitions: Vec<HealthTransitionEvent>,
}

impl TelemetrySnapshot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.session.is_none()
            && self.resource_statuses.is_empty()
            && self.resource_startups.is_empty()
            && self.host.is_none()
            && self.docker.is_empty()
            && self.failures.is_empty()
            && self.builds.is_empty()
            && self.health_transitions.is_empty()
    }

    pub fn total_metrics_count(&self) -> usize {
        (self.session.is_some() as usize)
            + self.resource_statuses.len()
            + self.resource_startups.len()
            + (self.host.is_some() as usize)
            + self.docker.len()
            + self.failures.len()
            + self.builds.len()
            + self.health_transitions.len()
    }
}

