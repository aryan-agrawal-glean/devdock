//! Configuration file synchronization to remote workspaces.

use crate::{RemoteError, Result};
use std::path::Path;
use std::process::Command;
use tracing::{debug, info};

/// Sync profile YAML to remote workspace.
pub fn sync_profile(workspace: &str, profile_path: &Path) -> Result<()> {
    info!("Syncing profile to {}: {:?}", workspace, profile_path);
    
    if !profile_path.exists() {
        return Err(RemoteError::ConfigSyncError(format!(
            "Profile not found: {:?}",
            profile_path
        )));
    }
    
    // Create target directory on remote first
    let _ = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg("mkdir")
        .arg("-p")
        .arg("/home/workspace/scio/devdock/profiles/")
        .output();
    
    // Use scp with coder SSH integration (format: workspace.coder:path)
    let output = Command::new("scp")
        .arg("-q")
        .arg("-o").arg("StrictHostKeyChecking=no")
        .arg("-o").arg("UserKnownHostsFile=/dev/null")
        .arg(profile_path)
        .arg(format!("{}.coder:/home/workspace/scio/devdock/profiles/", workspace))
        .output()
        .map_err(|e| RemoteError::ConfigSyncError(format!("scp failed: {}", e)))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RemoteError::ConfigSyncError(stderr.to_string()));
    }
    
    debug!("Profile synced successfully");
    Ok(())
}

/// Sync custom.ini to remote workspace if it exists.
pub fn sync_custom_ini(workspace: &str, custom_ini_path: Option<&Path>) -> Result<()> {
    if let Some(path) = custom_ini_path {
        if path.exists() {
            info!("Syncing custom.ini to {}", workspace);
            
            // Create config directory on remote
            let _ = Command::new("coder")
                .arg("ssh")
                .arg(workspace)
                .arg("--")
                .arg("mkdir")
                .arg("-p")
                .arg("/home/workspace/scio/config/")
                .output();
            
            let output = Command::new("scp")
                .arg("-q")
                .arg("-o").arg("StrictHostKeyChecking=no")
                .arg("-o").arg("UserKnownHostsFile=/dev/null")
                .arg(path)
                .arg(format!("{}.coder:/home/workspace/scio/config/", workspace))
                .output()
                .map_err(|e| RemoteError::ConfigSyncError(format!("scp failed: {}", e)))?;
            
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(RemoteError::ConfigSyncError(stderr.to_string()));
            }
            
            debug!("custom.ini synced successfully");
        }
    }
    Ok(())
}

/// Sync gcloud credentials to remote workspace.
pub fn sync_gcloud_credentials(workspace: &str) -> Result<()> {
    info!("Syncing gcloud credentials to {}", workspace);
    
    // Check if gcloud credentials exist locally
    let gcloud_dir = dirs::home_dir()
        .ok_or_else(|| RemoteError::ConfigSyncError("Cannot determine home directory".to_string()))?
        .join(".config/gcloud");
    
    if !gcloud_dir.exists() {
        debug!("No gcloud credentials found locally, skipping sync");
        return Ok(());
    }
    
    // Create gcloud config directory on remote
    let _ = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg("mkdir")
        .arg("-p")
        .arg("~/.config/gcloud/")
        .output();
    
    // Sync gcloud config directory using scp with recursion
    let output = Command::new("scp")
        .arg("-r")
        .arg("-q")
        .arg("-o").arg("StrictHostKeyChecking=no")
        .arg("-o").arg("UserKnownHostsFile=/dev/null")
        .arg(format!("{}/.config/gcloud/", dirs::home_dir().unwrap().display()))
        .arg(format!("{}.coder:~/.config/", workspace))
        .output()
        .map_err(|e| RemoteError::ConfigSyncError(format!("gcloud scp failed: {}", e)))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RemoteError::ConfigSyncError(stderr.to_string()));
    }
    
    debug!("gcloud credentials synced successfully");
    Ok(())
}

/// Sync all configuration files to remote workspace.
pub fn sync_all_config(
    workspace: &str,
    profile_path: &Path,
    custom_ini_path: Option<&Path>,
) -> Result<()> {
    sync_profile(workspace, profile_path)?;
    sync_custom_ini(workspace, custom_ini_path)?;
    sync_gcloud_credentials(workspace)?;
    Ok(())
}

