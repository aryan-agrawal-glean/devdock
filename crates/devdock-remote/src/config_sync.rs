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
        debug!("No ~/.config/gcloud found; skipping gcloud sync");
        return Ok(());
    }
    
    // Create temp directory for staging
    let temp_dir = tempfile::tempdir()
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to create temp directory: {}", e)))?;
    let temp_path = temp_dir.path();
    let gcloud_temp = temp_path.join("gcloud");
    
    std::fs::create_dir_all(&gcloud_temp)
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to create gcloud temp dir: {}", e)))?;
    std::fs::create_dir_all(gcloud_temp.join("configurations"))
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to create configurations dir: {}", e)))?;
    
    // Core files needed for gcloud auth
    let gcloud_files = vec![
        "application_default_credentials.json",
        "access_tokens.db",
        "credentials.db",
        "active_config",
        "configurations/config_default",
        "env.yaml",
    ];
    
    // Copy files to temp directory
    info!("Staging gcloud credential files...");
    for file in &gcloud_files {
        let src = gcloud_dir.join(file);
        if src.exists() {
            let dest = gcloud_temp.join(file);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to create parent dir: {}", e)))?;
            }
            std::fs::copy(&src, &dest)
                .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to copy {}: {}", file, e)))?;
            
            // Set permissions to 0600
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&dest)
                    .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to get metadata: {}", e)))?
                    .permissions();
                perms.set_mode(0o600);
                std::fs::set_permissions(&dest, perms)
                    .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to set permissions: {}", e)))?;
            }
            
            debug!("Staged file: {}", file);
        }
    }
    
    // Create remote directory
    debug!("Creating remote staging directory...");
    let output = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg("mkdir")
        .arg("-p")
        .arg("~/.secrets-sync")
        .output()
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to create remote directory: {}", e)))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RemoteError::ConfigSyncError(format!("Failed to create remote directory: {}", stderr)));
    }
    
    // Copy to workspace using scp
    info!("Copying gcloud credentials to remote...");
    let gcloud_temp_str = gcloud_temp.to_string_lossy();
    let scp_target = format!("{}.coder:~/.secrets-sync/", workspace);
    
    let output = Command::new("scp")
        .arg("-q")
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("UserKnownHostsFile=/dev/null")
        .arg("-r")
        .arg(gcloud_temp_str.as_ref())
        .arg(&scp_target)
        .output()
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to run scp: {}", e)))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RemoteError::ConfigSyncError(format!("Failed to copy gcloud credentials: {}", stderr)));
    }
    
    // Move to final location on remote
    debug!("Moving credentials to final locations...");
    let move_script = r#"
        # Copy to home directory for gcloud CLI
        mkdir -p ~/.config/gcloud
        rsync -a ~/.secrets-sync/gcloud/ ~/.config/gcloud/ 2>/dev/null

        # Also copy to /tmp/.config for services
        mkdir -p /tmp/.config/gcloud/configurations
        rsync -a ~/.secrets-sync/gcloud/ /tmp/.config/gcloud/ 2>/dev/null

        # Clean up staging directory
        rm -rf ~/.secrets-sync

        # Set proper permissions
        chmod -R 600 ~/.config/gcloud/* /tmp/.config/gcloud/* 2>/dev/null || true
        chmod 700 ~/.config/gcloud ~/.config/gcloud/configurations 2>/dev/null || true
        chmod 700 /tmp/.config/gcloud /tmp/.config/gcloud/configurations 2>/dev/null || true
    "#;
    
    let output = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg("bash")
        .arg("-c")
        .arg(move_script)
        .output()
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to move credentials: {}", e)))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!("Warning during credential move: {}", stderr);
    }
    
    // Verify gcloud auth
    debug!("Verifying gcloud configuration...");
    let output = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg("gcloud")
        .arg("config")
        .arg("configurations")
        .arg("list")
        .output()
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to verify gcloud: {}", e)))?;
    
    if output.status.success() {
        info!("gcloud credentials synced and verified");
    } else {
        debug!("gcloud credentials copied but configuration not verified");
    }
    
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

