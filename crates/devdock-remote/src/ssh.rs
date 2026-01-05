//! SSH utilities for remote workspace communication.

use crate::{RemoteError, Result};
use std::process::Command;
use tracing::{debug, info};

/// Execute a command on a remote workspace via SSH.
pub fn ssh_exec(workspace: &str, command: &str) -> Result<String> {
    info!("Executing on {}: {}", workspace, command);
    
    let output = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg("bash")
        .arg("-c")
        .arg(command)
        .output()
        .map_err(|e| RemoteError::SshError(format!("Failed to execute SSH command: {}", e)))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RemoteError::SshError(format!("Command failed: {}", stderr)));
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    debug!("SSH output: {}", stdout);
    Ok(stdout)
}

/// Check if SSH connection to workspace is available.
pub fn check_ssh_connectivity(workspace: &str) -> Result<bool> {
    match ssh_exec(workspace, "echo 'test'") {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Wait for SSH to become available (with timeout).
pub fn wait_for_ssh_ready(workspace: &str, timeout_secs: u64) -> Result<()> {
    use std::thread::sleep;
    use std::time::Duration;
    
    info!("Waiting for SSH connectivity to {}", workspace);
    
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    
    loop {
        if check_ssh_connectivity(workspace)? {
            info!("SSH ready for {}", workspace);
            return Ok(());
        }
        
        if start.elapsed() > timeout {
            return Err(RemoteError::SshError(format!(
                "SSH connection timeout after {} seconds",
                timeout_secs
            )));
        }
        
        sleep(Duration::from_secs(5));
    }
}

