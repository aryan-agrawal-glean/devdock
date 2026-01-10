//! Session state management for startup monitoring.
//!
//! Tracks session timing and manages state files to prevent re-running
//! the startup monitor when startup has already completed.

use crate::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info};

const STATE_FILE: &str = ".devdock/startup-monitor.pid";
const TIMING_FILE: &str = ".devdock/session_timing.txt";

/// Session timing metrics for analytics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTiming {
    /// When the devdock command was started (epoch seconds)
    pub command_start_time: Option<u64>,
    /// When tilt first responded (epoch seconds)
    pub tilt_up_time: Option<u64>,
    /// When the monitor started watching (epoch seconds)
    pub watch_start_time: u64,
    /// When startup completed (epoch seconds)
    pub startup_complete_time: Option<u64>,
    /// Total startup duration in seconds
    pub total_startup_seconds: Option<f64>,
    /// Tilt bootstrap duration in seconds
    pub tilt_bootstrap_seconds: Option<f64>,
}

impl SessionTiming {
    pub fn new() -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

        SessionTiming {
            command_start_time: Self::read_command_start_time(),
            tilt_up_time: None,
            watch_start_time: now,
            startup_complete_time: None,
            total_startup_seconds: None,
            tilt_bootstrap_seconds: None,
        }
    }

    fn read_command_start_time() -> Option<u64> {
        let home = dirs::home_dir()?;
        let timing_file = home.join(TIMING_FILE);
        fs::read_to_string(timing_file).ok()?.trim().parse().ok()
    }

    /// Mark tilt as ready (first successful tilt command).
    pub fn mark_tilt_up(&mut self) {
        if self.tilt_up_time.is_none() {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            self.tilt_up_time = Some(now);

            if let Some(cmd_start) = self.command_start_time {
                self.tilt_bootstrap_seconds = Some((now - cmd_start) as f64);
            }
        }
    }

    /// Mark startup as complete.
    pub fn mark_complete(&mut self) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        self.startup_complete_time = Some(now);
        self.total_startup_seconds = Some((now - self.watch_start_time) as f64);
    }

    /// Get the devdock_id from the state database.
    pub fn get_devdock_id() -> Option<String> {
        std::env::var("DEVDOCK_ID").ok()
    }

    /// Get machine identifier.
    pub fn get_machine_id() -> String {
        if let Ok(contents) = fs::read_to_string("/etc/machine-id") {
            return contents.trim().to_string();
        }

        #[cfg(unix)]
        {
            use std::ffi::CStr;
            let mut name = [0i8; 256];
            unsafe {
                if libc::gethostname(name.as_mut_ptr(), 256) == 0 {
                    if let Ok(hostname) = CStr::from_ptr(name.as_ptr()).to_str() {
                        return hostname.to_string();
                    }
                }
            }
        }

        "unknown".to_string()
    }
}

impl Default for SessionTiming {
    fn default() -> Self {
        Self::new()
    }
}

/// Manages startup state file to track if monitoring has completed.
#[derive(Debug)]
pub struct SessionState {
    state_file: PathBuf,
}

impl SessionState {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        SessionState { state_file: home.join(STATE_FILE) }
    }

    /// Check if startup has already completed (state file exists).
    pub fn is_completed(&self) -> bool {
        self.state_file.exists()
    }

    /// Mark startup as complete by writing state file.
    pub fn mark_success(&self) -> Result<()> {
        if let Some(parent) = self.state_file.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.state_file, "Startup complete")?;
        info!("Marked startup as complete: {:?}", self.state_file);
        Ok(())
    }

    /// Mark startup as failed.
    pub fn mark_failure(&self) -> Result<()> {
        if let Some(parent) = self.state_file.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.state_file, "Startup failed")?;
        info!("Marked startup as failed: {:?}", self.state_file);
        Ok(())
    }

    /// Clear the state file (for restart).
    pub fn clear(&self) -> Result<()> {
        if self.state_file.exists() {
            fs::remove_file(&self.state_file)?;
            debug!("Cleared startup state file");
        }
        Ok(())
    }

    /// Clean up session timing file.
    pub fn cleanup_timing(&self) -> Result<()> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let timing_file = home.join(TIMING_FILE);
        if timing_file.exists() {
            fs::remove_file(timing_file)?;
        }
        Ok(())
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_timing_new() {
        let timing = SessionTiming::new();
        assert!(timing.watch_start_time > 0);
        assert!(timing.startup_complete_time.is_none());
    }

    #[test]
    fn test_get_machine_id() {
        let id = SessionTiming::get_machine_id();
        assert!(!id.is_empty());
    }
}
