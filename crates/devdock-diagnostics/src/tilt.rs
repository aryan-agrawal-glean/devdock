//! Tilt snapshot capture functionality.

use crate::Result;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};
use uuid::Uuid;

/// Capture a Tilt snapshot if Tilt is running.
pub fn capture_tilt_snapshot(snapshot_dir: &Path) -> Result<Option<PathBuf>> {
    // Check if Tilt is running
    if let Some(pid) = crate::docker::check_tilt_running()? {
        info!("Tilt is running (PID: {}), capturing snapshot", pid);
    } else {
        warn!("Tilt is not running, skipping snapshot");
        return Ok(None);
    }

    // Generate snapshot filename
    let snapshot_id = format!("{}.json", Uuid::new_v4());
    let snapshot_path = snapshot_dir.join(&snapshot_id);

    // Capture snapshot using Tilt CLI
    let output =
        Command::new("tilt").args(["snapshot", "create", snapshot_path.to_str().unwrap()]).output();

    match output {
        Ok(output) if output.status.success() => {
            info!("Created Tilt snapshot: {:?}", snapshot_path);
            Ok(Some(snapshot_path))
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to create Tilt snapshot: {}", stderr);
            Ok(None)
        }
        Err(e) => {
            warn!("Failed to run tilt command: {}", e);
            Ok(None)
        }
    }
}
