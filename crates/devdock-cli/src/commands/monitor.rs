//! Monitor command implementation for startup monitoring.

use crate::output;
use anyhow::Result;
use devdock_monitor::{StartupMonitor, StartupMonitorConfig, StartupResult};

/// Execute the monitor command - runs startup monitoring until all resources are healthy.
pub async fn execute(timeout: Option<u64>, interval: Option<u64>, no_notify: bool) -> Result<()> {
    use devdock_monitor::StabilityConfig;
    use std::time::Duration;

    output::info("Starting resource monitor...");

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

    match monitor.run().await? {
        StartupResult::Success { duration_secs, resources } => {
            output::success(&format!(
                "All {} resources healthy in {}s",
                resources.len(),
                duration_secs
            ));
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
            anyhow::bail!("Startup monitoring timed out")
        }
        StartupResult::Failed { duration_secs, failed } => {
            output::warn(&format!(
                "Failed after {}s: {} resource(s) in error state",
                duration_secs,
                failed.len()
            ));
            for name in &failed {
                output::warn(&format!("  - {}", name));
            }
            anyhow::bail!("Resources failed to become healthy")
        }
        StartupResult::TiltNotRunning => {
            output::warn("Tilt is not running. Start DevDock first with 'devdock start <profile>'");
            anyhow::bail!("Tilt is not running")
        }
    }
}
