//! Health monitoring for sync daemon.

use crate::{Result, SyncError};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Health monitor for sync daemon.
pub struct HealthMonitor {
    workspace_name: String,
    last_sync: Option<Instant>,
    failed_syncs: u32,
    max_failures: u32,
}

impl HealthMonitor {
    /// Create a new health monitor.
    pub fn new(workspace_name: String) -> Self {
        Self {
            workspace_name,
            last_sync: None,
            failed_syncs: 0,
            max_failures: 100,  // Much more lenient - allow many retries
        }
    }
    
    /// Record a successful sync.
    pub fn record_success(&mut self) {
        self.last_sync = Some(Instant::now());
        self.failed_syncs = 0;
        debug!("Sync successful");
    }
    
    /// Record a failed sync.
    pub fn record_failure(&mut self) {
        self.failed_syncs += 1;
        warn!("Sync failed (failure count: {})", self.failed_syncs);
    }
    
    /// Check SSH connectivity to workspace.
    pub fn check_ssh_connectivity(&self) -> Result<bool> {
        use std::process::Command;
        
        let output = Command::new("coder")
            .arg("ssh")
            .arg(&self.workspace_name)
            .arg("--")
            .arg("echo")
            .arg("test")
            .output()
            .map_err(|e| SyncError::HealthCheckFailed(format!("SSH check failed: {}", e)))?;
        
        Ok(output.status.success())
    }
    
    /// Get sync lag (time since last successful sync).
    pub fn sync_lag(&self) -> Option<Duration> {
        self.last_sync.map(|last| last.elapsed())
    }
    
    /// Determine if daemon should restart.
    pub fn should_restart(&self) -> bool {
        // Restart if too many consecutive failures
        if self.failed_syncs >= self.max_failures {
            warn!("Too many consecutive failures ({}), should restart", self.failed_syncs);
            return true;
        }
        
        // NOTE: We don't check sync lag for event-driven file watchers
        // Sync lag check only makes sense for periodic/polling sync systems
        // For file watchers, no changes = healthy idle state, not a failure
        
        false
    }
    
    /// Check overall health.
    pub fn is_healthy(&self) -> bool {
        !self.should_restart()
    }
}

