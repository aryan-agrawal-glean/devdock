//! Configuration file collection.

use crate::Result;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::warn;

/// Collect DevDock configuration files.
pub fn get_config_files() -> Result<HashMap<String, String>> {
    let mut config_files = HashMap::new();

    // Get DEVDOCK_ROOT or fallback to current directory
    let devdock_root = std::env::var("DEVDOCK_ROOT")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            // Fallback: try current directory (matches Python behavior)
            std::env::current_dir().ok()
        });

    if let Some(devdock_root) = devdock_root {
        // Check for devdock.ini
        let devdock_ini = devdock_root.join("devdock/devdock.ini");
        if let Ok(content) = fs::read_to_string(&devdock_ini) {
            config_files.insert("devdock.ini".to_string(), content);
        } else {
            warn!("Could not read devdock.ini at {:?}", devdock_ini);
        }

        // Check for temp_profile.yaml
        let temp_profile = devdock_root.join("devdock/runtime_generated/temp_profile.yaml");
        if let Ok(content) = fs::read_to_string(&temp_profile) {
            config_files.insert("temp_profile.yaml".to_string(), content);
        } else {
            warn!("Could not read temp_profile.yaml at {:?}", temp_profile);
        }

        // Check for config/custom.ini (in workspace root)
        let custom_ini = devdock_root.join("config/custom.ini");
        if let Ok(content) = fs::read_to_string(&custom_ini) {
            config_files.insert("config/custom.ini".to_string(), content);
        } else {
            warn!("Could not read config/custom.ini at {:?}", custom_ini);
        }
    }

    Ok(config_files)
}

/// Get current git branch name.
pub fn get_current_git_branch() -> Result<String> {
    use std::process::Command;

    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()?;

    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(branch)
    } else {
        Ok("unknown".to_string())
    }
}
