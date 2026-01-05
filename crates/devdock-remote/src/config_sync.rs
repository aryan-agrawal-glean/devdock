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
    
    // Copy files by piping through coder ssh stdin (simple and reliable)
    info!("Copying gcloud credential files...");
    
    for file in &gcloud_files {
        let local_file = gcloud_dir.join(file);
        if !local_file.exists() {
            debug!("Skipping missing gcloud file: {}", file);
            continue;
        }
        
        // Read file content
        let content = match std::fs::read(&local_file) {
            Ok(c) => c,
            Err(e) => {
                debug!("Failed to read {}: {}", file, e);
                continue;
            }
        };
        
        // Pipe content directly to remote via coder ssh stdin
        // NOTE: No "--" separator! It breaks stdin piping
        let remote_path = format!("~/.config/gcloud/{}", file);
        let mut child = Command::new("coder")
            .arg("ssh")
            .arg(workspace)
            .arg(format!("bash -c 'cat > {}'", remote_path))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to start transfer for {}: {}", file, e)))?;
        
        // Write content to stdin
        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            stdin.write_all(&content)
                .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to write {}: {}", file, e)))?;
        }
        
        let output = child.wait_with_output()
            .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to transfer {}: {}", file, e)))?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!("Failed to copy gcloud file {}: {}", file, stderr);
        } else {
            debug!("Copied gcloud file: {}", file);
        }
    }
    
    // Set the correct gcloud account and project
    info!("Configuring gcloud account...");
    
    // Get account from application_default_credentials.json if it exists
    let creds_file = gcloud_dir.join("application_default_credentials.json");
    if creds_file.exists() {
        if let Ok(content) = std::fs::read_to_string(&creds_file) {
            // Try to extract account from JSON (simple string search, not full JSON parsing)
            if let Some(account_start) = content.find("\"account\"") {
                if let Some(value_start) = content[account_start..].find(":") {
                    let after_colon = &content[account_start + value_start + 1..];
                    if let Some(email) = extract_json_string_value(after_colon) {
                        if !email.is_empty() && email.contains("@") {
                            // Set the account
                            let _ = Command::new("coder")
                                .arg("ssh")
                                .arg(workspace)
                                .arg("--")
                                .arg("gcloud")
                                .arg("config")
                                .arg("set")
                                .arg("account")
                                .arg(&email)
                                .output();
                            debug!("Set gcloud account to: {}", email);
                        }
                    }
                }
            }
        }
    }
    
    // Get project from active_config if it exists
    let config_file = gcloud_dir.join("configurations/config_default");
    if config_file.exists() {
        if let Ok(content) = std::fs::read_to_string(&config_file) {
            // Parse INI-style config for project
            for line in content.lines() {
                if line.starts_with("project = ") {
                    let project = line.trim_start_matches("project = ").trim();
                    let _ = Command::new("coder")
                        .arg("ssh")
                        .arg(workspace)
                        .arg("--")
                        .arg("gcloud")
                        .arg("config")
                        .arg("set")
                        .arg("project")
                        .arg(project)
                        .output();
                    debug!("Set gcloud project to: {}", project);
                    break;
                }
            }
        }
    }
    
    debug!("gcloud credentials synced successfully");
    Ok(())
}

/// Extract a JSON string value (simple parser, no dependencies)
fn extract_json_string_value(s: &str) -> Option<String> {
    let s = s.trim_start();
    if !s.starts_with('"') {
        return None;
    }
    let s = &s[1..]; // Skip opening quote
    if let Some(end_idx) = s.find('"') {
        Some(s[..end_idx].to_string())
    } else {
        None
    }
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

