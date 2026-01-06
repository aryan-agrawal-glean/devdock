//! Rsync client for file synchronization.

use crate::{Result, SyncError};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::io::Write;
use tracing::{debug, info};

/// Rsync client for syncing files to remote workspace.
pub struct RsyncClient {
    workspace_name: String,
    local_path: PathBuf,
    remote_path: PathBuf,
}

impl RsyncClient {
    /// Create a new rsync client.
    pub fn new(workspace_name: String, local_path: PathBuf, remote_path: PathBuf) -> Self {
        Self {
            workspace_name,
            local_path,
            remote_path,
        }
    }
    
    /// Sync specific files to remote.
    pub fn sync_files(&self, files: &[PathBuf], full_mode: bool) -> Result<()> {
        if files.is_empty() {
            debug!("No files to sync");
            return Ok(());
        }
        
        info!("Syncing {} files to {}", files.len(), self.workspace_name);
        
        let mut cmd = Command::new("rsync");
        cmd.arg("-avz")
            .arg("--relative");
        
        if full_mode {
            cmd.arg("--delete");
        }
        
        // Add files-from stdin and rsync over SSH (coder sets up .ssh/config)
        cmd.arg("--files-from=-")
            .arg(&self.local_path)
            .arg(format!(
                "{}.coder:{}",
                self.workspace_name,
                self.remote_path.display()
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        
        debug!("Running rsync: {:?}", cmd);
        
        let mut child = cmd.spawn()
            .map_err(|e| SyncError::RsyncFailed(format!("Failed to spawn rsync: {}", e)))?;
        
        // Write file list to stdin
        if let Some(mut stdin) = child.stdin.take() {
            for file in files {
                writeln!(stdin, "{}", file.display())
                    .map_err(|e| SyncError::RsyncFailed(format!("Failed to write to rsync stdin: {}", e)))?;
            }
        }
        
        let output = child.wait_with_output()
            .map_err(|e| SyncError::RsyncFailed(format!("Failed to wait for rsync: {}", e)))?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SyncError::RsyncFailed(stderr.to_string()));
        }
        
        debug!("Rsync completed successfully");
        Ok(())
    }
    
    /// Sync entire directory to remote (full mode).
    pub fn sync_all(&self) -> Result<()> {
        info!("Syncing all files to {}", self.workspace_name);
        
        let output = Command::new("rsync")
            .arg("-avz")
            .arg("--delete")
            .arg("--exclude=.git")
            .arg("--exclude=node_modules")
            .arg("--exclude=bazel-*")
            .arg(format!("{}/", self.local_path.display()))
            .arg(format!(
                "{}.coder:{}",
                self.workspace_name,
                self.remote_path.display()
            ))
            .output()
            .map_err(|e| SyncError::RsyncFailed(format!("Failed to run rsync: {}", e)))?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SyncError::RsyncFailed(stderr.to_string()));
        }
        
        info!("Full sync completed");
        Ok(())
    }
}

