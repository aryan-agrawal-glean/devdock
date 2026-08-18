//! Telemetry collector for gathering metrics from Tilt, Docker, and the host system.

use crate::{
    emitter::TelemetryEmitter,
    models::{
        DockerMetrics, HealthTransitionEvent, HostMetrics, ResourceStartupMetrics,
        ResourceStatusMetrics, SessionMetrics, TelemetrySnapshot,
    },
    Result,
};
use chrono::{DateTime, Utc};
use devdock_monitor::{SessionTiming, TiltResource, TiltSnapshotBroker, DEFAULT_TILT_ENDPOINT};
use serde::Deserialize;
use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Configuration for the telemetry collector.
#[derive(Debug, Clone)]
pub struct TelemetryCollectorConfig {
    /// Explicit loopback Tilt web endpoint.
    pub tilt_endpoint: String,
    /// Collection interval for periodic metrics (default: 30 seconds)
    pub collection_interval: Duration,
    /// Whether to collect host metrics
    pub collect_host_metrics: bool,
    /// Whether to collect Docker metrics
    pub collect_docker_metrics: bool,
    /// Unhealthy count threshold before capturing logs (default: 5)
    pub unhealthy_threshold: u32,
    /// DevDock ID for this session
    pub devdock_id: String,
    /// Profile information
    pub profile: String,
    pub project: String,
    pub platform: String,
}

impl Default for TelemetryCollectorConfig {
    fn default() -> Self {
        TelemetryCollectorConfig {
            tilt_endpoint: DEFAULT_TILT_ENDPOINT.to_string(),
            collection_interval: Duration::from_secs(30),
            collect_host_metrics: true,
            collect_docker_metrics: true,
            unhealthy_threshold: 5,
            devdock_id: String::new(),
            profile: "unknown".to_string(),
            project: "unknown".to_string(),
            platform: "unknown".to_string(),
        }
    }
}

/// Tracks per-resource state for telemetry.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
struct ResourceTracker {
    first_seen: Option<DateTime<Utc>>,
    first_healthy: Option<DateTime<Utc>>,
    stable_at: Option<DateTime<Utc>>,
    last_status: Option<String>,
    last_status_change: Option<DateTime<Utc>>,
    unhealthy_count: u32,
    restart_count: i32,
    error_count: i32,
}

/// Telemetry collector that gathers metrics from various sources.
pub struct TelemetryCollector {
    config: TelemetryCollectorConfig,
    tilt_broker: TiltSnapshotBroker,
    emitters: Vec<Arc<dyn TelemetryEmitter>>,
    resource_trackers: RwLock<HashMap<String, ResourceTracker>>,
    session_start: DateTime<Utc>,
    machine_id: String,
}

impl TelemetryCollector {
    pub fn new(config: TelemetryCollectorConfig) -> Self {
        let tilt_broker =
            TiltSnapshotBroker::new(&config.tilt_endpoint, config.collection_interval)
                .expect("validated Tilt endpoint");
        Self::with_broker(config, tilt_broker)
    }

    pub fn with_broker(config: TelemetryCollectorConfig, tilt_broker: TiltSnapshotBroker) -> Self {
        if config.devdock_id.is_empty() {
            tracing::warn!("TelemetryCollector created with empty devdock_id — metrics will lack session correlation");
        }
        TelemetryCollector {
            config,
            tilt_broker,
            emitters: Vec::new(),
            resource_trackers: RwLock::new(HashMap::new()),
            session_start: Utc::now(),
            machine_id: SessionTiming::get_machine_id(),
        }
    }

    /// Add a telemetry emitter.
    pub fn add_emitter(&mut self, emitter: Arc<dyn TelemetryEmitter>) {
        self.emitters.push(emitter);
    }

    /// Collect a complete telemetry snapshot.
    pub async fn collect_snapshot(&self) -> Result<TelemetrySnapshot> {
        let mut snapshot = TelemetrySnapshot::new();

        if let Ok(resources) = self.tilt_broker.monitored_resources() {
            snapshot.resource_statuses = self.collect_resource_statuses(&resources).await;
            snapshot.health_transitions = self.check_health_transitions(&resources).await;
        }

        if self.config.collect_host_metrics {
            snapshot.host = self.collect_host_metrics().await.ok();
        }

        if self.config.collect_docker_metrics {
            snapshot.docker = self.collect_docker_metrics().await.unwrap_or_default();
        }

        Ok(snapshot)
    }

    /// Collect final startup metrics when session completes.
    pub async fn collect_startup_completion(
        &self,
        status: &str,
        healthy_count: usize,
        failed_count: usize,
        total_count: usize,
    ) -> Result<TelemetrySnapshot> {
        let mut snapshot = TelemetrySnapshot::new();
        let now = Utc::now();

        let total_startup_seconds = (now - self.session_start).num_milliseconds() as f64 / 1000.0;

        snapshot.session = Some(SessionMetrics {
            devdock_id: self.config.devdock_id.clone(),
            machine_id: self.machine_id.clone(),
            architecture: std::env::consts::ARCH.to_string(),
            platform: self.config.platform.clone(),
            project: self.config.project.clone(),
            profile: self.config.profile.clone(),
            branch: Self::get_git_branch(),
            binary_type: "rust".to_string(),
            binary_version: env!("CARGO_PKG_VERSION").to_string(),
            command_start_time: self.session_start,
            tilt_up_time: Some(self.session_start),
            watch_start_time: Some(self.session_start),
            startup_complete_time: Some(now),
            tilt_bootstrap_seconds: None,
            total_startup_seconds: Some(total_startup_seconds),
            startup_status: status.to_string(),
            total_resources: total_count as i32,
            healthy_resources: healthy_count as i32,
            failed_resources: failed_count as i32,
            timestamp: now,
        });

        if let Ok(resources) = self.tilt_broker.monitored_resources() {
            snapshot.resource_startups = self.collect_resource_startups(&resources).await;
        }

        if self.config.collect_host_metrics {
            snapshot.host = self.collect_host_metrics().await.ok();
        }

        if self.config.collect_docker_metrics {
            snapshot.docker = self.collect_docker_metrics().await.unwrap_or_default();
        }

        Ok(snapshot)
    }

    /// Emit collected metrics to all emitters.
    pub async fn emit(&self, snapshot: &TelemetrySnapshot) -> Result<()> {
        for emitter in &self.emitters {
            if !emitter.is_ready().await {
                warn!("Emitter {} not ready, skipping", emitter.name());
                continue;
            }

            match emitter.emit(snapshot).await {
                Ok(result) => {
                    if result.is_success() {
                        info!(
                            "Emitter {}: {} metrics emitted",
                            emitter.name(),
                            result.success_count
                        );
                    } else {
                        warn!(
                            "Emitter {}: {} success, {} failed: {:?}",
                            emitter.name(),
                            result.success_count,
                            result.failure_count,
                            result.errors
                        );
                    }
                }
                Err(e) => {
                    warn!("Emitter {} failed: {}", emitter.name(), e);
                }
            }
        }

        Ok(())
    }

    async fn collect_resource_statuses(
        &self,
        resources: &[TiltResource],
    ) -> Vec<ResourceStatusMetrics> {
        let now = Utc::now();
        let mut trackers = self.resource_trackers.write().await;

        resources
            .iter()
            .map(|r| {
                let tracker = trackers.entry(r.name.clone()).or_insert_with(|| ResourceTracker {
                    first_seen: Some(now),
                    ..Default::default()
                });

                if tracker.first_seen.is_none() {
                    tracker.first_seen = Some(now);
                }

                ResourceStatusMetrics {
                    devdock_id: self.config.devdock_id.clone(),
                    machine_id: self.machine_id.clone(),
                    timestamp: now,
                    resource_name: r.name.clone(),
                    resource_type: format!("{:?}", r.resource_type),
                    status: format!("{:?}", r.status),
                }
            })
            .collect()
    }

    async fn collect_resource_startups(
        &self,
        resources: &[TiltResource],
    ) -> Vec<ResourceStartupMetrics> {
        let trackers = self.resource_trackers.read().await;

        resources
            .iter()
            .map(|r| {
                let tracker = trackers.get(&r.name);

                let time_to_first_healthy = tracker
                    .and_then(|t| t.first_healthy)
                    .and_then(|fh| tracker.and_then(|t| t.first_seen).map(|fs| fh - fs))
                    .map(|d| d.num_milliseconds() as f64 / 1000.0);

                let time_to_stable = tracker
                    .and_then(|t| t.stable_at)
                    .and_then(|sa| tracker.and_then(|t| t.first_seen).map(|fs| sa - fs))
                    .map(|d| d.num_milliseconds() as f64 / 1000.0);

                ResourceStartupMetrics {
                    devdock_id: self.config.devdock_id.clone(),
                    name: r.name.clone(),
                    resource_type: format!("{:?}", r.resource_type),
                    bootup_time: None,
                    time_to_first_healthy,
                    time_to_stable,
                    state: format!("{:?}", r.status),
                    build_id: None,
                    load_order: 0,
                    creation_time: tracker.and_then(|t| t.first_seen),
                    restart_count: tracker.map_or(0, |t| t.restart_count),
                    error_count: tracker.map_or(0, |t| t.error_count),
                }
            })
            .collect()
    }

    async fn check_health_transitions(
        &self,
        resources: &[TiltResource],
    ) -> Vec<HealthTransitionEvent> {
        let now = Utc::now();
        let mut trackers = self.resource_trackers.write().await;
        let mut transitions = Vec::new();

        for resource in resources {
            let current_status = format!("{:?}", resource.status);

            let tracker =
                trackers.entry(resource.name.clone()).or_insert_with(ResourceTracker::default);

            if let Some(last) = &tracker.last_status {
                if last != &current_status {
                    let duration = tracker
                        .last_status_change
                        .map(|lsc| (now - lsc).num_milliseconds() as f64 / 1000.0);

                    transitions.push(HealthTransitionEvent {
                        devdock_id: self.config.devdock_id.clone(),
                        resource_name: resource.name.clone(),
                        from_status: last.clone(),
                        to_status: current_status.clone(),
                        timestamp: now,
                        duration_in_previous_state_seconds: duration,
                    });

                    if current_status == "Healthy" && tracker.first_healthy.is_none() {
                        tracker.first_healthy = Some(now);
                    }

                    if current_status == "Failed" {
                        tracker.error_count += 1;
                    }
                }
            }

            tracker.last_status = Some(current_status);
            tracker.last_status_change = Some(now);
        }

        transitions
    }

    async fn collect_host_metrics(&self) -> Result<HostMetrics> {
        let (cpu, mem_used, mem_total, mem_pct) = self.get_memory_stats();
        let (disk_used, disk_total, disk_pct) = self.get_disk_stats();
        let load = self.get_load_average();

        Ok(HostMetrics {
            devdock_id: self.config.devdock_id.clone(),
            machine_id: self.machine_id.clone(),
            platform: self.config.platform.clone(),
            architecture: std::env::consts::ARCH.to_string(),
            project: self.config.project.clone(),
            profile: self.config.profile.clone(),
            cpu_percent: cpu,
            memory_used_bytes: mem_used,
            memory_total_bytes: mem_total,
            memory_percent: mem_pct,
            disk_used_bytes: disk_used,
            disk_total_bytes: disk_total,
            disk_percent: disk_pct,
            load_average_1m: load.map(|(l, _, _)| l),
            load_average_5m: load.map(|(_, l, _)| l),
            load_average_15m: load.map(|(_, _, l)| l),
            timestamp: Utc::now(),
        })
    }

    async fn collect_docker_metrics(&self) -> Result<Vec<DockerMetrics>> {
        let output = Command::new("docker")
            .args(["stats", "--no-stream", "--format", "{{json .}}"])
            .output()?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let now = Utc::now();
        let mut metrics = Vec::new();

        for line in stdout.lines() {
            if line.is_empty() {
                continue;
            }

            if let Ok(stats) = serde_json::from_str::<DockerStatsJson>(line) {
                metrics.push(DockerMetrics {
                    devdock_id: self.config.devdock_id.clone(),
                    machine_id: self.machine_id.clone(),
                    container_id: stats.id,
                    name: stats.name,
                    cpu_percent: Self::parse_percent(&stats.cpu_perc),
                    memory_usage_bytes: Self::parse_memory_usage(&stats.mem_usage),
                    memory_limit_bytes: Self::parse_memory_limit(&stats.mem_usage),
                    memory_percent: Self::parse_percent(&stats.mem_perc),
                    network_input_bytes: Self::parse_io_input(&stats.net_io),
                    network_output_bytes: Self::parse_io_output(&stats.net_io),
                    block_input_bytes: Self::parse_io_input(&stats.block_io),
                    block_output_bytes: Self::parse_io_output(&stats.block_io),
                    pids: stats.pids.parse().unwrap_or(0),
                    timestamp: now,
                });
            }
        }

        Ok(metrics)
    }

    fn get_git_branch() -> Option<String> {
        Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }

    fn get_memory_stats(&self) -> (f64, u64, u64, f64) {
        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = Command::new("vm_stat").output() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let mut pages = HashMap::new();

                for line in stdout.lines() {
                    if let Some((key, val)) = line.split_once(':') {
                        if let Ok(v) = val.trim().trim_end_matches('.').parse::<u64>() {
                            pages.insert(key.to_string(), v);
                        }
                    }
                }

                let page_size = 4096u64;
                let free = pages.get("Pages free").copied().unwrap_or(0) * page_size;
                let active = pages.get("Pages active").copied().unwrap_or(0) * page_size;
                let inactive = pages.get("Pages inactive").copied().unwrap_or(0) * page_size;
                let wired = pages.get("Pages wired down").copied().unwrap_or(0) * page_size;

                let total = free + active + inactive + wired;
                let used = active + wired;
                let pct = if total > 0 { (used as f64 / total as f64) * 100.0 } else { 0.0 };

                return (0.0, used, total, pct);
            }
        }

        #[cfg(target_os = "linux")]
        {
            if let Ok(contents) = std::fs::read_to_string("/proc/meminfo") {
                let mut total = 0u64;
                let mut available = 0u64;

                for line in contents.lines() {
                    if line.starts_with("MemTotal:") {
                        total = Self::parse_proc_value(line);
                    } else if line.starts_with("MemAvailable:") {
                        available = Self::parse_proc_value(line);
                    }
                }

                let used = total.saturating_sub(available);
                let pct = if total > 0 { (used as f64 / total as f64) * 100.0 } else { 0.0 };

                return (0.0, used, total, pct);
            }
        }

        (0.0, 0, 0, 0.0)
    }

    #[cfg(target_os = "linux")]
    fn parse_proc_value(line: &str) -> u64 {
        line.split_whitespace()
            .nth(1)
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v * 1024)
            .unwrap_or(0)
    }

    fn get_disk_stats(&self) -> (u64, u64, f64) {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        let devdock_path = home.join(".devdock");

        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::mem::MaybeUninit;

            let path = devdock_path.to_string_lossy();
            if let Ok(cpath) = CString::new(path.as_bytes()) {
                let mut stat: MaybeUninit<libc::statvfs> = MaybeUninit::uninit();
                unsafe {
                    if libc::statvfs(cpath.as_ptr(), stat.as_mut_ptr()) == 0 {
                        let stat = stat.assume_init();
                        #[allow(clippy::unnecessary_cast)]
                        let total = u64::from(stat.f_blocks) * (stat.f_frsize as u64);
                        #[allow(clippy::unnecessary_cast)]
                        let free = u64::from(stat.f_bfree) * (stat.f_frsize as u64);
                        let used = total - free;
                        let pct =
                            if total > 0 { (used as f64 / total as f64) * 100.0 } else { 0.0 };
                        return (used, total, pct);
                    }
                }
            }
        }

        (0, 0, 0.0)
    }

    fn get_load_average(&self) -> Option<(f64, f64, f64)> {
        #[cfg(unix)]
        {
            let mut loadavg: [f64; 3] = [0.0; 3];
            unsafe {
                if libc::getloadavg(loadavg.as_mut_ptr(), 3) == 3 {
                    return Some((loadavg[0], loadavg[1], loadavg[2]));
                }
            }
        }
        None
    }

    fn parse_percent(s: &str) -> f64 {
        s.trim_end_matches('%').parse().unwrap_or(0.0)
    }

    fn parse_memory_usage(s: &str) -> u64 {
        s.split('/').next().map_or(0, Self::parse_size)
    }

    fn parse_memory_limit(s: &str) -> u64 {
        s.split('/').nth(1).map_or(0, Self::parse_size)
    }

    fn parse_io_input(s: &str) -> u64 {
        s.split('/').next().map_or(0, Self::parse_size)
    }

    fn parse_io_output(s: &str) -> u64 {
        s.split('/').nth(1).map_or(0, Self::parse_size)
    }

    fn parse_size(s: &str) -> u64 {
        let s = s.trim();
        let (num, unit) = if let Some(idx) = s.find(|c: char| !c.is_ascii_digit() && c != '.') {
            (&s[..idx], &s[idx..])
        } else {
            (s, "")
        };

        let value: f64 = num.parse().unwrap_or(0.0);
        let multiplier = match unit.trim().to_uppercase().as_str() {
            "B" => 1.0,
            "KIB" | "KB" | "K" => 1024.0,
            "MIB" | "MB" | "M" => 1024.0 * 1024.0,
            "GIB" | "GB" | "G" => 1024.0 * 1024.0 * 1024.0,
            _ => 1.0,
        };

        (value * multiplier) as u64
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerStatsJson {
    #[serde(rename = "ID")]
    id: String,
    name: String,
    #[serde(rename = "CPUPerc")]
    cpu_perc: String,
    #[serde(rename = "MemUsage")]
    mem_usage: String,
    #[serde(rename = "MemPerc")]
    mem_perc: String,
    #[serde(rename = "NetIO")]
    net_io: String,
    #[serde(rename = "BlockIO")]
    block_io: String,
    #[serde(rename = "PIDs")]
    pids: String,
}
