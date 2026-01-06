//! DevDock remote workspace management.
//!
//! This crate provides functionality for managing DevDock instances on remote
//! Coder workspaces, including workspace provisioning, port forwarding, and
//! configuration synchronization.

pub mod coder;
pub mod config_sync;
pub mod port_forward;
pub mod ssh;
pub mod workspace;

pub use coder::{CoderWorkspaceConfig, check_coder_installed, workspace_exists};
pub use port_forward::{PortForwardManager, PortMapping, PortType};
pub use workspace::{cleanup_existing_devdock, provision_workspace, start_remote_devdock};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum RemoteError {
    #[error("Coder CLI not found. Please install coder: https://coder.com/install")]
    CoderNotInstalled,

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

