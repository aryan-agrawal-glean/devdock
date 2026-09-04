//! DevDock remote workspace management.
//!
//! This crate provides functionality for managing DevDock instances on remote
//! Coder workspaces, including workspace provisioning, port forwarding, and
//! configuration synchronization.

pub mod coder;
pub mod config_sync;
pub mod diag;
pub mod port_forward;
pub mod ssh;
pub mod workspace;

pub use coder::{check_coder_installed, workspace_exists, CoderWorkspaceConfig};
pub use diag::{
    probe_remote_tilt, RemoteTiltProbe, REMOTE_START_LOG, REMOTE_STATUS_SSH_TIMEOUT_SECS,
};
pub use port_forward::{
    managed_port_forward_status, start_managed_port_forward, start_managed_port_forward_logged,
    stop_managed_port_forward, ManagedForwardStatus, ManagedForwardStopOutcome, PortForwardManager,
    PortMapping, PortType,
};
pub use workspace::{
    cleanup_existing_devdock, inspect_remote_git_state, provision_workspace, start_remote_devdock,
    RemoteGitState,
};

use std::process::Command;
use thiserror::Error;
use tracing::debug;

#[derive(Error, Debug)]
pub enum RemoteError {
    #[error("Coder CLI not found. Please install coder: https://coder.com/install")]
    CoderNotInstalled,

    #[error(
        "gcloud CLI not found. Please install gcloud: https://cloud.google.com/sdk/docs/install"
    )]
    GcloudNotInstalled,

    #[error("gcloud auth is missing or expired. Run `gcloud auth login --update-adc` and retry your DevDock command.")]
    GcloudAuthFailed,

    #[error("Workspace not found: {0}")]
    WorkspaceNotFound(String),

    #[error("Workspace creation failed: {0}")]
    WorkspaceCreationFailed(String),

    #[error("SSH connection failed: {0}")]
    SshError(String),

    #[error("SSH command timed out after {0} seconds")]
    SshTimeout(u64),

    #[error("Port forwarding failed: {0}")]
    PortForwardError(String),

    #[error("Configuration sync failed: {0}")]
    ConfigSyncError(String),

    #[error("Command execution failed: {0}")]
    CommandFailed(String),

    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, RemoteError>;

/// Verify that gcloud CLI is installed and authenticated locally.
///
/// This preflight check should be called before any remote DevDock operations
/// that require GCP authentication (workspace creation, config sync, etc.).
/// It fails fast with a clear error message if gcloud auth is missing or expired,
/// avoiding wasted time on workspace provisioning that would fail later.
///
/// # Errors
///
/// Returns `RemoteError::GcloudNotInstalled` if the gcloud CLI is not found.
/// Returns `RemoteError::GcloudAuthFailed` if `gcloud auth print-access-token` fails.
pub fn check_gcloud_auth() -> Result<()> {
    debug!("Checking gcloud CLI availability and authentication");

    // Check if gcloud CLI is installed
    if which::which("gcloud").is_err() {
        return Err(RemoteError::GcloudNotInstalled);
    }

    // Check if gcloud auth is valid by attempting to print an access token
    let output =
        Command::new("gcloud").args(["auth", "print-access-token"]).output().map_err(|e| {
            debug!("Failed to run gcloud auth print-access-token: {}", e);
            RemoteError::GcloudAuthFailed
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!("gcloud auth print-access-token failed: {}", stderr);
        return Err(RemoteError::GcloudAuthFailed);
    }

    // Check that we actually got a token (not empty output)
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        debug!("gcloud auth print-access-token returned empty output");
        return Err(RemoteError::GcloudAuthFailed);
    }

    debug!("gcloud auth check passed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_gcloud_auth_returns_error_types() {
        // This test verifies the error types exist and can be constructed
        let gcloud_not_installed = RemoteError::GcloudNotInstalled;
        assert!(gcloud_not_installed.to_string().contains("gcloud CLI not found"));

        let gcloud_auth_failed = RemoteError::GcloudAuthFailed;
        assert!(gcloud_auth_failed.to_string().contains("gcloud auth is missing or expired"));
        assert!(gcloud_auth_failed.to_string().contains("gcloud auth login --update-adc"));
    }
}
