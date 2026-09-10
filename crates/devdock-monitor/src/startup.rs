//! Startup monitoring for DevDock.
//!
//! Monitors Tilt resources during startup phase, waiting for all monitored
//! resources to become healthy or timing out after a configurable duration.

use crate::{
    HealthTracker, MonitorError, Notifier, ResourceStatus, Result, SessionState, SessionTiming,
    StabilityConfig, StabilityInfo, StatsCollector, TiltResource, TiltSnapshot, TiltSnapshotBroker,
    DEFAULT_TILT_ENDPOINT,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{debug, info, warn};

/// Configuration for the startup monitor.
#[derive(Debug, Clone)]
pub struct StartupMonitorConfig {
    /// Explicit loopback Tilt web endpoint.
    pub tilt_endpoint: String,
    /// Timeout in seconds (default: 1500 = 25 minutes)
    pub timeout_secs: u64,
    /// Polling interval in seconds (default: 2)
    pub poll_interval_secs: u64,
    /// Whether to send desktop notifications
    pub notifications_enabled: bool,
    /// Whether to show progress output
    pub show_progress: bool,
    /// Skip monitoring if already completed (state file exists)
    pub skip_if_completed: bool,
    /// Collect and log stats on completion
    pub collect_stats: bool,
    /// Health stability configuration
    pub stability: StabilityConfig,
}

impl Default for StartupMonitorConfig {
    fn default() -> Self {
        StartupMonitorConfig {
            tilt_endpoint: DEFAULT_TILT_ENDPOINT.to_string(),
            timeout_secs: 1500,
            poll_interval_secs: 2,
            notifications_enabled: true,
            show_progress: true,
            skip_if_completed: true,
            collect_stats: true,
            stability: StabilityConfig::default(),
        }
    }
}

/// Result of the startup monitoring phase.
#[derive(Debug, Clone)]
pub enum StartupResult {
    /// All resources became healthy.
    Success { duration_secs: u64, resources: Vec<String> },
    /// Timed out waiting for resources.
    Timeout { duration_secs: u64, healthy: Vec<String>, pending: Vec<String>, failed: Vec<String> },
    /// One or more resources failed permanently.
    Failed { duration_secs: u64, failed: Vec<String> },
    /// Tilt is not running.
    TiltNotRunning,
}

impl StartupResult {
    pub fn is_success(&self) -> bool {
        matches!(self, StartupResult::Success { .. })
    }
}

/// Startup monitor that waits for all resources to become healthy.
pub struct StartupMonitor {
    config: StartupMonitorConfig,
    tilt_broker: TiltSnapshotBroker,
    notifier: Notifier,
    session_state: SessionState,
    stats_collector: StatsCollector,
    health_tracker: RefCell<HealthTracker>,
}

impl StartupMonitor {
    pub fn new(config: StartupMonitorConfig) -> Self {
        let tilt_broker = TiltSnapshotBroker::new(
            &config.tilt_endpoint,
            Duration::from_secs(config.poll_interval_secs),
        )
        .expect("validated Tilt endpoint");
        Self::with_broker(config, tilt_broker)
    }

    pub fn with_broker(config: StartupMonitorConfig, tilt_broker: TiltSnapshotBroker) -> Self {
        let notifier = Notifier::with_enabled(config.notifications_enabled);
        let health_tracker = HealthTracker::new(config.stability.clone());
        let stats_collector = StatsCollector::new(tilt_broker.clone());
        StartupMonitor {
            config,
            tilt_broker,
            notifier,
            session_state: SessionState::new(),
            stats_collector,
            health_tracker: RefCell::new(health_tracker),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(StartupMonitorConfig::default())
    }

    /// Clear the state file to allow monitoring to run again.
    pub fn clear_state(&self) -> Result<()> {
        self.session_state.clear()
    }

    async fn refresh_tilt_snapshot(&self) -> Result<Arc<TiltSnapshot>> {
        let broker = self.tilt_broker.clone();
        tokio::task::spawn_blocking(move || broker.refresh()).await.map_err(|error| {
            MonitorError::TiltHttp(format!("Tilt snapshot task failed: {error}"))
        })?
    }

    async fn tilt_snapshot(&self) -> Result<Arc<TiltSnapshot>> {
        let broker = self.tilt_broker.clone();
        tokio::task::spawn_blocking(move || broker.snapshot()).await.map_err(|error| {
            MonitorError::TiltHttp(format!("Tilt snapshot task failed: {error}"))
        })?
    }

    /// Wait for Tilt to become available.
    async fn wait_for_tilt(&self, max_wait: Duration) -> Option<Arc<TiltSnapshot>> {
        let start = Instant::now();
        let check_interval = Duration::from_secs(1);

        if self.config.show_progress {
            println!("Waiting for Tilt to start...");
        }

        while start.elapsed() < max_wait {
            if let Ok(snapshot) = self.refresh_tilt_snapshot().await {
                info!("Tilt is now available after {:?}", start.elapsed());
                return Some(snapshot);
            }
            debug!("Tilt not ready yet, waiting...");
            sleep(check_interval).await;
        }

        None
    }

    /// Run the startup monitoring loop.
    pub async fn run(&self) -> Result<StartupResult> {
        if self.config.skip_if_completed && self.session_state.is_completed() {
            info!("Startup already completed (state file exists), skipping monitor");
            if self.config.show_progress {
                println!("\x1B[32mStartup already completed. Skipping resource check.\x1B[0m");
            }
            return Ok(StartupResult::Success { duration_secs: 0, resources: vec![] });
        }

        info!(
            "Starting resource monitor (timeout: {}s, poll interval: {}s)",
            self.config.timeout_secs, self.config.poll_interval_secs
        );

        // Wait for Tilt to become available (up to 30 seconds)
        let Some(initial_snapshot) = self.wait_for_tilt(Duration::from_secs(30)).await else {
            warn!("Tilt did not become available within 30 seconds");
            return Ok(StartupResult::TiltNotRunning);
        };

        let mut session_timing = SessionTiming::new();
        session_timing.mark_tilt_up();

        let start = Instant::now();
        let timeout = Duration::from_secs(self.config.timeout_secs);
        let poll_interval = Duration::from_secs(self.config.poll_interval_secs);

        let mut resource_statuses: HashMap<String, ResourceStatus> = HashMap::new();

        loop {
            let elapsed = start.elapsed();

            if elapsed >= timeout {
                return self.handle_timeout(elapsed, &resource_statuses);
            }

            match self.tilt_snapshot().await {
                Ok(snapshot) => {
                    initial_snapshot.ensure_same_tilt_session(&snapshot)?;
                    let resources = snapshot
                        .resources
                        .iter()
                        .filter(|resource| resource.should_monitor())
                        .cloned()
                        .collect::<Vec<_>>();
                    let total = resources.len();

                    if total == 0 {
                        if self.config.show_progress {
                            println!("No monitored resources found yet...");
                        }
                        sleep(poll_interval).await;
                        continue;
                    }

                    self.update_statuses(&resources, &mut resource_statuses);

                    let (stable_healthy, stabilizing, pending, failed) =
                        self.categorize_resources(&resources);

                    if self.config.show_progress {
                        self.print_status_summary(
                            &stable_healthy,
                            &stabilizing,
                            &pending,
                            &failed,
                            total,
                        );
                    }

                    // All resources are stably healthy
                    if stable_healthy.len() == total {
                        return self.handle_success(elapsed, stable_healthy);
                    }

                    // Don't fail immediately - resources often briefly show as failed
                    // during startup then recover. Only fail at timeout.
                }
                Err(MonitorError::TiltNotRunning) => {
                    warn!("Tilt stopped running");
                    return Ok(StartupResult::TiltNotRunning);
                }
                Err(e) => {
                    warn!("Error querying Tilt: {}", e);
                }
            }

            sleep(poll_interval).await;
        }
    }

    fn update_statuses(
        &self,
        resources: &[TiltResource],
        statuses: &mut HashMap<String, ResourceStatus>,
    ) {
        let mut tracker = self.health_tracker.borrow_mut();
        for resource in resources {
            // Record raw status in tracker for stability analysis
            tracker.record(&resource.name, resource.status.clone());
            statuses.insert(resource.name.clone(), resource.status.clone());
        }
    }

    fn categorize_resources(
        &self,
        resources: &[TiltResource],
    ) -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
        let tracker = self.health_tracker.borrow();
        let mut stable_healthy = Vec::new();
        let mut stabilizing = Vec::new();
        let mut pending = Vec::new();
        let mut failed = Vec::new();

        for resource in resources {
            match resource.status {
                ResourceStatus::Failed => failed.push(resource.name.clone()),
                ResourceStatus::Healthy => {
                    // Check if stably healthy
                    if tracker.is_stable(&resource.name) {
                        stable_healthy.push(resource.name.clone());
                    } else {
                        stabilizing.push(resource.name.clone());
                    }
                }
                _ => pending.push(resource.name.clone()),
            }
        }

        (stable_healthy, stabilizing, pending, failed)
    }

    #[allow(dead_code)]
    fn get_stability_info(&self, name: &str) -> Option<StabilityInfo> {
        self.health_tracker.borrow().get_stability_info(name)
    }

    fn print_status_summary(
        &self,
        stable_healthy: &[String],
        stabilizing: &[String],
        pending: &[String],
        failed: &[String],
        total: usize,
    ) {
        // Simple single-line output that doesn't conflict with Tilt's output
        let stable = stable_healthy.len();
        let stab = stabilizing.len();
        let pend = pending.len();
        let fail = failed.len();

        // Clear line and print compact status
        print!("\r\x1B[K");

        if stable == total {
            println!("\x1B[32m✓ All {stable}/{total} resources stable\x1B[0m");
        } else {
            print!("\x1B[34m[{stable}/{total}]\x1B[0m ");

            if stab > 0 {
                print!("\x1B[36m{stab}◐\x1B[0m ");
            }
            if pend > 0 {
                print!("\x1B[33m{pend}⟳\x1B[0m ");
            }
            if fail > 0 {
                print!("\x1B[31m{fail}✗\x1B[0m ");
            }

            // Show what's not stable yet
            let not_stable: Vec<&str> = pending
                .iter()
                .chain(stabilizing.iter())
                .chain(failed.iter())
                .take(3)
                .map(std::string::String::as_str)
                .collect();

            if !not_stable.is_empty() {
                print!("waiting: {}", not_stable.join(", "));
                if pending.len() + stabilizing.len() + failed.len() > 3 {
                    print!("...");
                }
            }

            io::stdout().flush().ok();
        }
    }

    #[allow(dead_code)]
    fn truncate_list(&self, items: &[String], max: usize) -> String {
        if items.len() <= max {
            items.join(", ")
        } else {
            let shown: Vec<_> = items.iter().take(max).cloned().collect();
            format!("{}, ... (+{} more)", shown.join(", "), items.len() - max)
        }
    }

    fn handle_success(&self, elapsed: Duration, healthy: Vec<String>) -> Result<StartupResult> {
        if self.config.show_progress {
            println!();
            println!(
                "\x1B[32m\x1B[1m✓ All {} resources stable in {}s\x1B[0m",
                healthy.len(),
                elapsed.as_secs()
            );
        }

        info!("Startup complete: {} resources healthy in {:?}", healthy.len(), elapsed);

        if let Err(e) = self.session_state.mark_success() {
            warn!("Failed to write success state: {}", e);
        }

        if self.config.collect_stats {
            self.collect_and_log_stats();
        }

        self.notifier.success("DevDock startup complete...");

        Ok(StartupResult::Success { duration_secs: elapsed.as_secs(), resources: healthy })
    }

    fn collect_and_log_stats(&self) {
        match self.stats_collector.collect_host_stats() {
            Ok(stats) => {
                debug!(
                    "Host stats: CPU={:.1}%, Memory={:.1}%, Disk={:.1}%",
                    stats.cpu_percent, stats.memory_percent, stats.disk_percent
                );
            }
            Err(e) => {
                debug!("Failed to collect host stats: {}", e);
            }
        }

        match self.stats_collector.collect_resource_stats() {
            Ok(stats) => {
                debug!("Collected stats for {} resources", stats.len());
            }
            Err(e) => {
                debug!("Failed to collect resource stats: {}", e);
            }
        }
    }

    fn handle_timeout(
        &self,
        elapsed: Duration,
        statuses: &HashMap<String, ResourceStatus>,
    ) -> Result<StartupResult> {
        let tracker = self.health_tracker.borrow();
        let mut stable_healthy = Vec::new();
        let mut stabilizing = Vec::new();
        let mut pending = Vec::new();
        let mut failed = Vec::new();

        for (name, status) in statuses {
            match status {
                ResourceStatus::Healthy => {
                    if tracker.is_stable(name) {
                        stable_healthy.push(name.clone());
                    } else {
                        stabilizing.push(name.clone());
                    }
                }
                ResourceStatus::Failed => failed.push(name.clone()),
                _ => pending.push(name.clone()),
            }
        }
        drop(tracker);

        if self.config.show_progress {
            println!();
            println!(
                "\x1B[31m\x1B[1m✗ Startup timed out after {} seconds\x1B[0m",
                self.config.timeout_secs
            );
            println!(
                "  Stable: {}, Stabilizing: {}, Pending: {}, Failed: {}",
                stable_healthy.len(),
                stabilizing.len(),
                pending.len(),
                failed.len()
            );
            if !stabilizing.is_empty() || !pending.is_empty() {
                let waiting: Vec<_> = stabilizing.iter().chain(pending.iter()).take(5).collect();
                println!(
                    "  Waiting on: {}",
                    waiting.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                );
            }
            if !failed.is_empty() {
                println!(
                    "  Failed: {}",
                    failed.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
                );
            }
        }

        let total = stable_healthy.len() + stabilizing.len() + pending.len() + failed.len();
        warn!("Startup timeout: {}/{} stable after {:?}", stable_healthy.len(), total, elapsed);

        if let Err(e) = self.session_state.mark_failure() {
            warn!("Failed to write failure state: {}", e);
        }

        if self.config.collect_stats {
            self.collect_and_log_stats();
        }

        self.notifier.failure("DevDock startup timed out...");

        // Combine stabilizing with pending for the result
        pending.extend(stabilizing);

        Ok(StartupResult::Timeout {
            duration_secs: elapsed.as_secs(),
            healthy: stable_healthy,
            pending,
            failed,
        })
    }

    #[allow(dead_code)]
    fn handle_failure(&self, elapsed: Duration, failed: Vec<String>) -> Result<StartupResult> {
        if self.config.show_progress {
            println!();
            println!(
                "\x1B[31m\x1B[1m✗ Startup failed: {} resource(s) in error state\x1B[0m",
                failed.len()
            );
            println!("  Failed: {}", failed.join(", "));
        }

        warn!("Startup failed: resources {:?} in error state", failed);

        if let Err(e) = self.session_state.mark_failure() {
            warn!("Failed to write failure state: {}", e);
        }

        if self.config.collect_stats {
            self.collect_and_log_stats();
        }

        self.notifier.failure("DevDock startup failed...");

        Ok(StartupResult::Failed { duration_secs: elapsed.as_secs(), failed })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    fn tilt_view(start_time: &str) -> String {
        format!(r#"{{"tiltStartTime":"{start_time}","uiResources":[]}}"#)
    }

    fn serve_tilt_views(views: Vec<String>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for body in views {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        format!("http://{address}/")
    }

    #[test]
    fn test_config_default() {
        let config = StartupMonitorConfig::default();
        assert_eq!(config.timeout_secs, 1500);
        assert_eq!(config.poll_interval_secs, 2);
        assert!(config.notifications_enabled);
    }

    #[test]
    fn test_startup_result_is_success() {
        let success =
            StartupResult::Success { duration_secs: 60, resources: vec!["svc".to_string()] };
        assert!(success.is_success());

        let timeout = StartupResult::Timeout {
            duration_secs: 1500,
            healthy: vec![],
            pending: vec!["svc".to_string()],
            failed: vec![],
        };
        assert!(!timeout.is_success());
    }

    #[test]
    fn monitor_rejects_replacement_tilt_session() {
        let endpoint = serve_tilt_views(vec![
            tilt_view("2026-08-18T09:20:31Z"),
            tilt_view("2026-08-18T09:21:31Z"),
        ]);
        let config = StartupMonitorConfig {
            tilt_endpoint: endpoint.clone(),
            timeout_secs: 1,
            poll_interval_secs: 0,
            notifications_enabled: false,
            show_progress: false,
            skip_if_completed: false,
            collect_stats: false,
            stability: StabilityConfig::default(),
        };
        let broker = TiltSnapshotBroker::new(endpoint, Duration::ZERO).unwrap();
        let monitor = StartupMonitor::with_broker(config, broker);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

        assert!(matches!(runtime.block_on(monitor.run()), Err(MonitorError::TiltRestarted)));
    }
}
