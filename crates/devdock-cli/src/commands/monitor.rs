//! Monitor command implementation for startup monitoring.

use crate::output;
use anyhow::Result;
use devdock_monitor::{StartupMonitor, StartupMonitorConfig, StartupResult};

/// Execute the monitor command - runs startup monitoring until all resources are healthy.
pub async fn execute(
    timeout: Option<u64>,
    interval: Option<u64>,
    no_notify: bool,
    profile_arg: Option<String>,
) -> Result<()> {
    use devdock_monitor::StabilityConfig;
    use std::time::Duration;

    output::info("Starting resource monitor...");

    // Track start time for telemetry
    let monitor_start_time = chrono::Utc::now();

    let config = StartupMonitorConfig {
        timeout_secs: timeout.unwrap_or(1200),
        poll_interval_secs: interval.unwrap_or(2),
        notifications_enabled: !no_notify,
        show_progress: true,
        skip_if_completed: true, // Skip if already completed when running standalone
        collect_stats: true,
        stability: StabilityConfig {
            required_successes: 3,
            window_duration: Duration::from_secs(10),
            min_healthy_duration: Duration::from_secs(5),
        },
    };

    let monitor = StartupMonitor::new(config);

    // Get devdock_id (must be set by start.rs, fallback to new ID for standalone runs)
    let devdock_id = std::env::var("DEVDOCK_ID").unwrap_or_else(|_| {
        tracing::warn!("DEVDOCK_ID not set, generating new ID (correlation may break)");
        devdock_state::Instance::generate_id(devdock_state::SessionOrigin::Dev)
    });

    // Get profile name for telemetry (use CLI arg if provided, otherwise detect)
    let profile = profile_arg.unwrap_or_else(get_profile_name);

    match monitor.run().await? {
        StartupResult::Success { duration_secs, resources } => {
            output::success(&format!(
                "All {} resources healthy in {}s",
                resources.len(),
                duration_secs
            ));

            // Emit telemetry with resource details
            emit_telemetry_with_resources(
                &devdock_id,
                "success",
                resources.len(),
                0,
                &profile,
                monitor_start_time,
                duration_secs,
            )
            .await;

            Ok(())
        }
        StartupResult::Timeout { duration_secs, healthy, pending, failed } => {
            output::warn(&format!(
                "Timeout after {}s: {} healthy, {} pending, {} failed",
                duration_secs,
                healthy.len(),
                pending.len(),
                failed.len()
            ));

            // Emit telemetry with resource details
            emit_telemetry_with_resources(
                &devdock_id,
                "timeout",
                healthy.len(),
                failed.len(),
                &profile,
                monitor_start_time,
                duration_secs,
            )
            .await;

            anyhow::bail!("Startup monitoring timed out")
        }
        StartupResult::Failed { duration_secs, failed } => {
            output::warn(&format!(
                "Failed after {}s: {} resource(s) in error state",
                duration_secs,
                failed.len()
            ));
            for name in &failed {
                output::warn(&format!("  - {name}"));
            }

            // Emit telemetry with resource details
            emit_telemetry_with_resources(
                &devdock_id,
                "failed",
                0,
                failed.len(),
                &profile,
                monitor_start_time,
                duration_secs,
            )
            .await;

            anyhow::bail!("Resources failed to become healthy")
        }
        StartupResult::TiltNotRunning => {
            output::warn("Tilt is not running. Start DevDock first with 'devdock start <profile>'");

            // Emit telemetry for tilt_stopped
            emit_telemetry_with_resources(
                &devdock_id,
                "tilt_stopped",
                0,
                0,
                &profile,
                monitor_start_time,
                0,
            )
            .await;

            anyhow::bail!("Tilt is not running")
        }
    }
}

/// Get profile name from various sources (synchronous - can't use async here)
fn get_profile_name() -> String {
    // Method 1: Read from state file written by devdock start
    if let Ok(home) = std::env::var("HOME") {
        let state_file = std::path::PathBuf::from(home).join(".devdock/current_profile.txt");
        if let Ok(profile) = std::fs::read_to_string(&state_file) {
            let profile = profile.trim();
            if !profile.is_empty() {
                tracing::debug!("Profile from state file: {}", profile);
                return profile.to_string();
            }
        }
    }

    // Method 2: Check DEVDOCK_PROFILE environment variable
    if let Ok(profile) = std::env::var("DEVDOCK_PROFILE") {
        if !profile.is_empty() {
            tracing::debug!("Profile from env var: {}", profile);
            return profile;
        }
    }

    // Method 3: Try to read from temp_profile.yaml
    if let Ok(devdock_root) = std::env::var("DEVDOCK_ROOT") {
        let profile_path = format!("{}/devdock/runtime_generated/temp_profile.yaml", devdock_root);
        if let Ok(contents) = std::fs::read_to_string(&profile_path) {
            // Simple line parsing
            for line in contents.lines() {
                if line.trim().starts_with("profile:") {
                    if let Some(name) = line.split(':').nth(1) {
                        let name = name.trim().trim_matches('"').trim_matches('\'');
                        if !name.is_empty() {
                            tracing::debug!("Profile from temp_profile.yaml: {}", name);
                            return name.to_string();
                        }
                    }
                }
            }
        }
    }

    // Fallback to unknown
    tracing::debug!("Profile detection failed, using 'unknown'");
    "unknown".to_string()
}

/// Emit telemetry data to BigQuery with resource-level metrics
async fn emit_telemetry_with_resources(
    devdock_id: &str,
    status: &str,
    healthy_count: usize,
    failed_count: usize,
    profile: &str,
    monitor_start_time: chrono::DateTime<chrono::Utc>,
    duration_secs: u64,
) {
    use chrono::Utc;
    use devdock_monitor::TiltClient;
    use devdock_telemetry::{
        BigQueryConfig, BigQueryEmitter, ResourceStartupMetrics, ResourceStatusMetrics,
        SessionMetrics, TelemetryEmitter, TelemetrySnapshot,
    };

    let machine_id = get_machine_id();
    let now = Utc::now();

    // Calculate timing metrics
    let total_startup_secs = duration_secs as f64;

    // Build session metrics
    let session = SessionMetrics {
        devdock_id: devdock_id.to_string(),
        machine_id: machine_id.clone(),
        architecture: std::env::consts::OS.to_string(),
        platform: std::env::consts::OS.to_string(),
        project: "scio".to_string(),
        profile: profile.to_string(),
        branch: get_git_branch().ok(),
        binary_type: "rust".to_string(),
        binary_version: env!("CARGO_PKG_VERSION").to_string(),
        command_start_time: monitor_start_time,
        tilt_up_time: Some(monitor_start_time),
        watch_start_time: Some(monitor_start_time),
        startup_complete_time: Some(now),
        tilt_bootstrap_seconds: None,
        total_startup_seconds: Some(total_startup_secs),
        startup_status: status.to_string(),
        total_resources: (healthy_count + failed_count) as i32,
        healthy_resources: healthy_count as i32,
        failed_resources: failed_count as i32,
        timestamp: now,
    };

    // Collect per-resource metrics from Tilt
    let tilt_client = TiltClient::new();
    let mut resource_statuses = Vec::new();
    let mut resource_startups = Vec::new();

    if let Ok(resources) = tilt_client.get_monitored_resources() {
        // Collect resource status snapshots
        for resource in &resources {
            resource_statuses.push(ResourceStatusMetrics {
                devdock_id: devdock_id.to_string(),
                machine_id: machine_id.clone(),
                timestamp: now,
                resource_name: resource.name.clone(),
                resource_type: format!("{:?}", resource.resource_type),
                status: format!("{:?}", resource.status),
            });
        }

        // Collect resource startup metrics
        for resource in &resources {
            resource_startups.push(ResourceStartupMetrics {
                devdock_id: devdock_id.to_string(),
                name: resource.name.clone(),
                resource_type: format!("{:?}", resource.resource_type),
                bootup_time: resource.build_duration_secs, // Now extracted from buildHistory
                time_to_first_healthy: None,               // Would need tracking from start
                time_to_stable: None,
                state: format!("{:?}", resource.status),
                build_id: None,
                load_order: 0,
                creation_time: Some(now),
                restart_count: 0,
                error_count: 0,
            });
        }
    }

    let snapshot = TelemetrySnapshot {
        session: Some(session),
        resource_statuses,
        resource_startups,
        ..Default::default()
    };

    // Configure BigQuery emitter
    let config = BigQueryConfig {
        project_id: "dev-sandbox-334901".to_string(),
        dataset: "devdock".to_string(),
        use_adc: true,
        ..Default::default()
    };

    // Try to emit to BigQuery
    match BigQueryEmitter::new(config) {
        Ok(emitter) => match emitter.emit(&snapshot).await {
            Ok(result) => {
                if result.is_success() {
                    tracing::info!(
                        "Telemetry emitted to BigQuery: status={}, {} total metrics",
                        status,
                        result.success_count
                    );
                } else {
                    tracing::warn!("Partial telemetry emission: {} errors", result.failure_count);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to emit telemetry to BigQuery: {}", e);
            }
        },
        Err(e) => {
            tracing::debug!(
                "BigQuery telemetry not available: {} (this is normal for local dev)",
                e
            );
        }
    }
}

/// Get a stable machine identifier
fn get_machine_id() -> String {
    use std::ffi::CStr;

    #[cfg(unix)]
    {
        let mut name = [0 as libc::c_char; 256];
        unsafe {
            if libc::gethostname(name.as_mut_ptr(), 256) == 0 {
                if let Ok(hostname) = CStr::from_ptr(name.as_ptr()).to_str() {
                    return hostname.to_string();
                }
            }
        }
    }

    // Fallback to USER env var
    std::env::var("USER").unwrap_or_else(|_| "unknown".to_string())
}

/// Get current git branch name
fn get_git_branch() -> Result<String> {
    use std::process::Command;

    let output = Command::new("git").args(["rev-parse", "--abbrev-ref", "HEAD"]).output()?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Ok("no-branch".to_string())
    }
}
