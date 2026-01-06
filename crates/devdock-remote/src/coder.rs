//! Coder CLI wrapper for workspace management.

use crate::{RemoteError, Result};
use serde::{Deserialize, Serialize};
use std::process::{Command, Stdio, Child};
use tracing::{debug, info, warn};

/// Configuration for creating a Coder workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoderWorkspaceConfig {
    /// Coder template name (always "devdock-glean")
    pub template: String,
    /// Workspace name (user-provided or branch name)
    pub workspace_name: String,
    /// GCP region (default: us-east4-a)
    pub region: String,
    /// Local port offset (not passed to Coder, used for port forwarding calculation)
    pub port_base: Option<u16>,
    /// DevDock profile name
    pub profile: String,
}

/// Check if coder CLI is installed and available.
pub fn check_coder_installed() -> Result<()> {
    which::which("coder")
        .map_err(|_| RemoteError::CoderNotInstalled)?;
    
    debug!("Coder CLI found");
    Ok(())
}

/// Create a new Coder workspace.
pub fn create_workspace(config: &CoderWorkspaceConfig) -> Result<String> {
    check_coder_installed()?;
    
    info!("Creating Coder workspace: {}", config.workspace_name);
    
    let mut cmd = Command::new("coder");
    cmd.arg("create")
        .arg(&config.workspace_name)
        .arg("--template")
        .arg(&config.template)
        .arg("--parameter")
        .arg(format!("gcp_region={}", config.region))
        .arg("--parameter")
        .arg("jetbrains_ide=IU")  // IntelliJ IDEA Ultimate
        .arg("--parameter")
        .arg("dotfiles_uri=")  // Empty dotfiles
        .arg("--parameter")
        .arg("github_username=")  // Empty default
        .arg("--parameter")
        .arg("branch_name=master")  // Always use master for remote (file sync handles actual branch)
        .arg("--parameter")
        .arg("force_fresh_data=false")
        .arg("--yes"); // Non-interactive
    
    debug!("Running: {:?}", cmd);
    
    let output = cmd.output()
        .map_err(|e| RemoteError::CommandFailed(format!("Failed to run coder create: {}", e)))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RemoteError::WorkspaceCreationFailed(stderr.to_string()));
    }
    
    info!("Workspace created: {}", config.workspace_name);
    Ok(config.workspace_name.clone())
}

/// Start an existing Coder workspace.
pub fn start_workspace(name: &str) -> Result<String> {
    check_coder_installed()?;
    
    info!("Starting Coder workspace: {}", name);
    
    let output = Command::new("coder")
        .arg("start")
        .arg(name)
        .arg("--yes")
        .output()
        .map_err(|e| RemoteError::CommandFailed(format!("Failed to start workspace: {}", e)))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RemoteError::WorkspaceNotFound(stderr.to_string()));
    }
    
    info!("Workspace started: {}", name);
    Ok(name.to_string())
}

/// Stop a Coder workspace.
pub fn stop_workspace(name: &str) -> Result<()> {
    check_coder_installed()?;
    
    info!("Stopping Coder workspace: {}", name);
    
    let output = Command::new("coder")
        .arg("stop")
        .arg(name)
        .arg("--yes")
        .output()
        .map_err(|e| RemoteError::CommandFailed(format!("Failed to stop workspace: {}", e)))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!("Failed to stop workspace: {}", stderr);
    }
    
    Ok(())
}

/// Delete a Coder workspace.
pub fn delete_workspace(name: &str) -> Result<()> {
    check_coder_installed()?;
    
    info!("Deleting Coder workspace: {}", name);
    
    let output = Command::new("coder")
        .arg("delete")
        .arg(name)
        .arg("--yes")
        .output()
        .map_err(|e| RemoteError::CommandFailed(format!("Failed to delete workspace: {}", e)))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!("Failed to delete workspace: {}", stderr);
    }
    
    Ok(())
}

/// Check if a workspace exists.
pub fn workspace_exists(name: &str) -> Result<bool> {
    check_coder_installed()?;
    
    let output = Command::new("coder")
        .arg("list")
        .output()
        .map_err(|e| RemoteError::CommandFailed(format!("Failed to list workspaces: {}", e)))?;
    
    if !output.status.success() {
        return Ok(false);
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Check if workspace name appears in the list output
    // Format is like: "workspace-name    template-name    ..."
    Ok(stdout.lines().any(|line| line.split_whitespace().next() == Some(name)))
}

/// Get workspace status.
pub fn get_workspace_status(name: &str) -> Result<String> {
    check_coder_installed()?;
    
    let output = Command::new("coder")
        .arg("list")
        .arg("--output")
        .arg("json")
        .output()
        .map_err(|e| RemoteError::CommandFailed(format!("Failed to list workspaces: {}", e)))?;
    
    if !output.status.success() {
        return Err(RemoteError::CommandFailed("Failed to get workspace status".to_string()));
    }
    
    // Parse JSON output and find workspace
    let stdout = String::from_utf8_lossy(&output.stdout);
    // For now, return simple status - can be enhanced with proper JSON parsing
    Ok(if stdout.contains(name) { "running" } else { "stopped" }.to_string())
}

/// Start a single port forward for a workspace.
pub fn port_forward_single(workspace: &str, local_port: u16, remote_port: u16) -> Result<Child> {
    check_coder_installed()?;
    
    info!("Port forwarding {}:{} -> {}", workspace, remote_port, local_port);
    
    let child = Command::new("coder")
        .arg("port-forward")
        .arg(workspace)
        .arg("--tcp")
        .arg(format!("{}:{}", local_port, remote_port))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| RemoteError::PortForwardError(format!("Failed to start port forward: {}", e)))?;
    
    debug!("Port forward started with PID: {}", child.id());
    Ok(child)
}

/// Kill a port forward process by PID.
pub fn kill_port_forward(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::process::Command;
        let _ = Command::new("kill")
            .arg(pid.to_string())
            .output();
    }
    
    #[cfg(windows)]
    {
        use std::process::Command;
        let _ = Command::new("taskkill")
            .arg("/PID")
            .arg(pid.to_string())
            .arg("/F")
            .output();
    }
    
    Ok(())
}


