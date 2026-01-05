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
    debug!("Creating remote directory...");
    let _ = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg("mkdir")
        .arg("-p")
        .arg("/home/workspace/scio/devdock/profiles/")
        .output();
    
    // Use coder's built-in file copy instead of raw scp
    debug!("Copying profile using coder: {:?} -> {}", profile_path, workspace);
    
    // Read the file content
    let content = std::fs::read_to_string(profile_path)
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to read profile: {}", e)))?;
    
    // Write it to remote using cat via SSH
    let filename = profile_path.file_name()
        .ok_or_else(|| RemoteError::ConfigSyncError("Invalid profile path".to_string()))?
        .to_string_lossy();
    
    let write_cmd = format!(
        "cat > /home/workspace/scio/devdock/profiles/{} << 'DEVDOCK_EOF'\n{}\nDEVDOCK_EOF",
        filename, content
    );
    
    let output = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg("bash")
        .arg("-c")
        .arg(&write_cmd)
        .output()
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to copy profile: {}", e)))?;
    
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
            
            // Read and write file via SSH
            let content = std::fs::read_to_string(path)
                .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to read custom.ini: {}", e)))?;
            
            let write_cmd = format!(
                "cat > /home/workspace/scio/config/custom.ini << 'DEVDOCK_EOF'\n{}\nDEVDOCK_EOF",
                content
            );
            
            let output = Command::new("coder")
                .arg("ssh")
                .arg(workspace)
                .arg("--")
                .arg("bash")
                .arg("-c")
                .arg(&write_cmd)
                .output()
                .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to copy custom.ini: {}", e)))?;
            
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
        .arg("bash")
        .arg("-c")
        .arg("mkdir -p ~/.config/gcloud/configurations")
        .output();
    
    // Copy only essential gcloud files (matching Python implementation)
    let gcloud_files = vec![
        "application_default_credentials.json",
        "access_tokens.db",
        "credentials.db",
        "active_config",
        "configurations/config_default",
        "env.yaml",
    ];
    
    // Create tar archive and pipe to remote
    info!("Creating gcloud credentials archive...");
    
    let mut tar_cmd = Command::new("tar");
    tar_cmd
        .arg("-czf")
        .arg("-")
        .arg("-C")
        .arg(&gcloud_dir);
    
    // Add only files that exist
    for file in &gcloud_files {
        if gcloud_dir.join(file).exists() {
            tar_cmd.arg(file);
        } else {
            debug!("Skipping missing gcloud file: {}", file);
        }
    }
    
    let tar_output = tar_cmd
        .output()
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to create tar: {}", e)))?;
    
    if !tar_output.status.success() {
        return Err(RemoteError::ConfigSyncError(
            "Failed to create gcloud credentials archive".to_string()
        ));
    }
    
    // Pipe tar data to remote and extract
    let extract_cmd = "tar -xzf - -C ~/.config/gcloud/";
    
    let mut ssh_cmd = Command::new("coder");
    ssh_cmd
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg("bash")
        .arg("-c")
        .arg(extract_cmd)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                stdin.write_all(&tar_output.stdout)?;
            }
            child.wait()
        })
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to extract credentials on remote: {}", e)))?;
    
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

