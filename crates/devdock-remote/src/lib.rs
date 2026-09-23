//! DevDock remote workspace management.
//!
//! This crate provides functionality for managing DevDock instances on remote
//! Coder workspaces, including workspace provisioning, port forwarding, and
//! configuration synchronization.

pub mod coder;
pub mod config_sync;
pub mod port_forward;
mod process;
pub mod ssh;
pub mod workspace;

pub use coder::{check_coder_installed, workspace_exists, CoderWorkspaceConfig};
pub use port_forward::{
    managed_port_forward_status, start_managed_port_forward, start_managed_port_forward_logged,
    stop_managed_port_forward, ManagedForwardStatus, ManagedForwardStopOutcome, PortForwardManager,
    PortMapping, PortType,
};
pub use workspace::{
    inspect_remote_git_state, inspect_remote_runtime, provision_workspace,
    reconcile_remote_checkout, start_remote_devdock, sync_remote_configuration, RemoteGitState,
    RemoteRuntimeSnapshot,
};

use std::process::{Command, Stdio};
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

    #[error(
        "local gcloud {check} preflight failed for profile project '{project}'. Run `gcloud auth login --update-adc` and retry your DevDock command."
    )]
    GcloudPreflightFailed { check: &'static str, project: String },

    #[error("remote workspace checkout is not safe to reconcile: {0}")]
    RemoteCheckoutUnsafe(String),

    #[error("Workspace not found: {0}")]
    WorkspaceNotFound(String),

    #[error("Workspace creation failed: {0}")]
    WorkspaceCreationFailed(String),

    #[error("SSH connection failed: {0}")]
    SshError(String),

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

/// Verify local user auth, ADC, and access to the selected profile project.
///
/// This is deliberately status-only: stdout and stderr are discarded so an access
/// token never reaches DevDock logs. Call this before session claim, Coder lifecycle
/// commands, remote SSH, or credential transfer.
///
/// # Errors
///
/// Returns `RemoteError::GcloudNotInstalled` if the gcloud CLI is not found.
/// Returns `RemoteError::GcloudPreflightFailed` when one of the probes fails.
pub fn check_gcloud_preflight(project: &str) -> Result<()> {
    let project = project.trim();
    if project.is_empty() {
        return Err(RemoteError::ConfigSyncError(
            "selected profile has no usable project".to_string(),
        ));
    }

    debug!(project, "Checking local gcloud preflight");
    if which::which("gcloud").is_err() {
        return Err(RemoteError::GcloudNotInstalled);
    }

    check_gcloud_preflight_with(project, |args| {
        Command::new("gcloud")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

fn check_gcloud_preflight_with<F>(project: &str, mut run: F) -> Result<()>
where
    F: FnMut(&[&str]) -> bool,
{
    let probes: [(&str, Vec<&str>); 3] = [
        ("user authentication", vec!["auth", "print-access-token"]),
        (
            "application-default credentials",
            vec!["auth", "application-default", "print-access-token"],
        ),
        (
            "profile project access",
            vec!["projects", "describe", project, "--format=value(projectNumber)"],
        ),
    ];

    for (check, args) in probes {
        if !run(&args) {
            return Err(RemoteError::GcloudPreflightFailed { check, project: project.to_string() });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcloud_preflight_stops_at_the_first_failed_status_probe() {
        let mut seen = Vec::new();
        let error = check_gcloud_preflight_with("profile-project", |args| {
            seen.push(args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>());
            seen.len() == 1
        })
        .unwrap_err();

        assert_eq!(
            seen,
            vec![
                vec!["auth", "print-access-token"],
                vec!["auth", "application-default", "print-access-token"],
            ]
        );
        assert!(matches!(
            error,
            RemoteError::GcloudPreflightFailed {
                check: "application-default credentials",
                project,
            } if project == "profile-project"
        ));
    }

    #[test]
    fn gcloud_preflight_uses_the_profile_project_without_mutating_config() {
        let mut seen = Vec::new();
        check_gcloud_preflight_with("profile-project", |args| {
            seen.push(args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>());
            true
        })
        .unwrap();

        assert_eq!(
            seen.last().unwrap(),
            &vec!["projects", "describe", "profile-project", "--format=value(projectNumber)",]
        );
        assert!(seen.iter().all(|args| args.first().map_or(true, |arg| arg != "config")));
    }

    #[test]
    fn gcloud_preflight_errors_for_empty_profile_project() {
        assert!(matches!(
            check_gcloud_preflight("   "),
            Err(RemoteError::ConfigSyncError(message)) if message.contains("no usable project")
        ));
    }

    #[test]
    fn gcloud_preflight_error_mentions_the_local_reauth_command() {
        // This test verifies the error types exist and can be constructed
        let gcloud_not_installed = RemoteError::GcloudNotInstalled;
        assert!(gcloud_not_installed.to_string().contains("gcloud CLI not found"));

        let gcloud_auth_failed = RemoteError::GcloudPreflightFailed {
            check: "user authentication",
            project: "profile-project".to_string(),
        };
        assert!(gcloud_auth_failed.to_string().contains("gcloud auth login --update-adc"));
    }
}
