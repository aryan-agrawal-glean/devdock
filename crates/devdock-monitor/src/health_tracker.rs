//! Health stability tracking for Tilt resources.
//!
//! Implements a stability window to prevent false positives from brief healthy states.
//! A resource is only considered "stable healthy" after N consecutive healthy checks
//! within a time window.

use crate::ResourceStatus;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Configuration for health stability requirements.
#[derive(Debug, Clone)]
pub struct StabilityConfig {
    /// Number of consecutive healthy checks required (default: 3)
    pub required_successes: usize,
    /// Time window within which successes must occur (default: 10 seconds)
    pub window_duration: Duration,
    /// Minimum time a resource must remain healthy (default: 5 seconds)
    pub min_healthy_duration: Duration,
}

impl Default for StabilityConfig {
    fn default() -> Self {
        StabilityConfig {
            required_successes: 3,
            window_duration: Duration::from_secs(10),
            min_healthy_duration: Duration::from_secs(5),
        }
    }
}

/// A single health check observation.
#[derive(Debug, Clone)]
struct HealthObservation {
    status: ResourceStatus,
    timestamp: Instant,
}

/// Tracks health observations for a single resource.
#[derive(Debug)]
struct ResourceHealthHistory {
    observations: VecDeque<HealthObservation>,
    first_healthy_at: Option<Instant>,
    stable_since: Option<Instant>,
}

impl ResourceHealthHistory {
    fn new() -> Self {
        ResourceHealthHistory {
            observations: VecDeque::new(),
            first_healthy_at: None,
            stable_since: None,
        }
    }

    fn record(&mut self, status: ResourceStatus, config: &StabilityConfig) {
        let now = Instant::now();

        self.observations.push_back(HealthObservation {
            status: status.clone(),
            timestamp: now,
        });

        // Remove observations outside the window
        let cutoff = now - config.window_duration;
        while let Some(front) = self.observations.front() {
            if front.timestamp < cutoff {
                self.observations.pop_front();
            } else {
                break;
            }
        }

        // Track first healthy timestamp
        if status.is_healthy() {
            if self.first_healthy_at.is_none() {
                self.first_healthy_at = Some(now);
            }
        } else {
            // Reset if we see a non-healthy status
            self.first_healthy_at = None;
            self.stable_since = None;
        }
    }

    fn is_stable(&self, config: &StabilityConfig) -> bool {
        let now = Instant::now();

        // Count recent healthy observations
        let healthy_count = self
            .observations
            .iter()
            .filter(|obs| obs.status.is_healthy())
            .count();

        if healthy_count < config.required_successes {
            return false;
        }

        // Check if we have enough consecutive healthy checks
        let recent_all_healthy = self
            .observations
            .iter()
            .rev()
            .take(config.required_successes)
            .all(|obs| obs.status.is_healthy());

        if !recent_all_healthy {
            return false;
        }

        // Check minimum healthy duration
        if let Some(first_healthy) = self.first_healthy_at {
            if now.duration_since(first_healthy) >= config.min_healthy_duration {
                return true;
            }
        }

        false
    }

    fn consecutive_healthy_count(&self) -> usize {
        self.observations
            .iter()
            .rev()
            .take_while(|obs| obs.status.is_healthy())
            .count()
    }

    fn time_healthy(&self) -> Option<Duration> {
        self.first_healthy_at.map(|t| t.elapsed())
    }
}

/// Tracks health stability for all resources.
#[derive(Debug)]
pub struct HealthTracker {
    config: StabilityConfig,
    resources: HashMap<String, ResourceHealthHistory>,
}

impl HealthTracker {
    pub fn new(config: StabilityConfig) -> Self {
        HealthTracker {
            config,
            resources: HashMap::new(),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(StabilityConfig::default())
    }

    /// Record a health observation for a resource.
    pub fn record(&mut self, name: &str, status: ResourceStatus) {
        let history = self
            .resources
            .entry(name.to_string())
            .or_insert_with(ResourceHealthHistory::new);
        history.record(status, &self.config);
    }

    /// Check if a resource is stably healthy.
    pub fn is_stable(&self, name: &str) -> bool {
        self.resources
            .get(name)
            .map(|h| h.is_stable(&self.config))
            .unwrap_or(false)
    }

    /// Get the effective status for a resource (considering stability).
    pub fn get_stable_status(&self, name: &str, raw_status: &ResourceStatus) -> ResourceStatus {
        if raw_status.is_healthy() {
            if self.is_stable(name) {
                ResourceStatus::Healthy
            } else {
                // Still stabilizing - treat as pending
                ResourceStatus::Pending
            }
        } else {
            raw_status.clone()
        }
    }

    /// Get stability info for display.
    pub fn get_stability_info(&self, name: &str) -> Option<StabilityInfo> {
        self.resources.get(name).map(|h| StabilityInfo {
            consecutive_healthy: h.consecutive_healthy_count(),
            required: self.config.required_successes,
            time_healthy: h.time_healthy(),
            min_duration: self.config.min_healthy_duration,
            is_stable: h.is_stable(&self.config),
        })
    }

    /// Get all resources that are stably healthy.
    pub fn get_stable_resources(&self) -> Vec<String> {
        self.resources
            .iter()
            .filter(|(_, h)| h.is_stable(&self.config))
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Clear tracking for a specific resource.
    pub fn clear(&mut self, name: &str) {
        self.resources.remove(name);
    }

    /// Clear all tracking.
    pub fn clear_all(&mut self) {
        self.resources.clear();
    }
}

/// Stability information for display purposes.
#[derive(Debug, Clone)]
pub struct StabilityInfo {
    pub consecutive_healthy: usize,
    pub required: usize,
    pub time_healthy: Option<Duration>,
    pub min_duration: Duration,
    pub is_stable: bool,
}

impl StabilityInfo {
    pub fn progress_string(&self) -> String {
        if self.is_stable {
            "stable".to_string()
        } else if self.consecutive_healthy > 0 {
            let time_str = self
                .time_healthy
                .map(|d| format!("{:.1}s", d.as_secs_f64()))
                .unwrap_or_else(|| "0s".to_string());
            format!(
                "{}/{} checks, {}",
                self.consecutive_healthy, self.required, time_str
            )
        } else {
            "waiting".to_string()
        }
    }
}

impl Default for HealthTracker {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn test_stability_requires_multiple_checks() {
        let config = StabilityConfig {
            required_successes: 3,
            window_duration: Duration::from_secs(10),
            min_healthy_duration: Duration::from_millis(100),
        };
        let mut tracker = HealthTracker::new(config);

        // First healthy check - not stable yet
        tracker.record("svc1", ResourceStatus::Healthy);
        assert!(!tracker.is_stable("svc1"));

        // Second healthy check - still not stable
        tracker.record("svc1", ResourceStatus::Healthy);
        assert!(!tracker.is_stable("svc1"));

        // Third healthy check - need to wait for min duration
        tracker.record("svc1", ResourceStatus::Healthy);

        // Wait for min healthy duration
        sleep(Duration::from_millis(150));
        tracker.record("svc1", ResourceStatus::Healthy);

        assert!(tracker.is_stable("svc1"));
    }

    #[test]
    fn test_unhealthy_resets_stability() {
        let config = StabilityConfig {
            required_successes: 2,
            window_duration: Duration::from_secs(10),
            min_healthy_duration: Duration::from_millis(50),
        };
        let mut tracker = HealthTracker::new(config);

        tracker.record("svc1", ResourceStatus::Healthy);
        tracker.record("svc1", ResourceStatus::Healthy);
        sleep(Duration::from_millis(60));
        tracker.record("svc1", ResourceStatus::Healthy);
        assert!(tracker.is_stable("svc1"));

        // Unhealthy resets
        tracker.record("svc1", ResourceStatus::Failed);
        assert!(!tracker.is_stable("svc1"));

        // Need to build up again
        tracker.record("svc1", ResourceStatus::Healthy);
        assert!(!tracker.is_stable("svc1"));
    }

    #[test]
    fn test_get_stable_status() {
        let config = StabilityConfig {
            required_successes: 2,
            window_duration: Duration::from_secs(10),
            min_healthy_duration: Duration::from_millis(10),
        };
        let mut tracker = HealthTracker::new(config);

        // Raw healthy but not stable yet -> returns Pending
        tracker.record("svc1", ResourceStatus::Healthy);
        let status = tracker.get_stable_status("svc1", &ResourceStatus::Healthy);
        assert_eq!(status, ResourceStatus::Pending);

        // After enough checks and time -> returns Healthy
        tracker.record("svc1", ResourceStatus::Healthy);
        sleep(Duration::from_millis(20));
        tracker.record("svc1", ResourceStatus::Healthy);
        let status = tracker.get_stable_status("svc1", &ResourceStatus::Healthy);
        assert_eq!(status, ResourceStatus::Healthy);
    }
}
