//! File synchronization for remote DevDock workspaces.
//!
//! This crate provides real-time file synchronization between local and remote
//! workspaces using rsync and file watching.

pub mod daemon;
pub mod git;
pub mod health;
pub mod rsync;
pub mod watcher;

pub use daemon::SyncDaemon;
pub use git::{list_all_files, list_git_tracked_files};
pub use health::HealthMonitor;
pub use rsync::RsyncClient;
pub use watcher::FileWatcher;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SyncError {
    #[error("Git repository not found at {0}")]
    GitRepoNotFound(String),

    #[error("Rsync failed: {0}")]
    RsyncFailed(String),

    #[error("File watcher error: {0}")]
    WatcherError(String),

    #[error("Health check failed: {0}")]
    HealthCheckFailed(String),

    #[error("Daemon error: {0}")]
    DaemonError(String),

    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error(transparent)]
    GitError(#[from] git2::Error),

    #[error(transparent)]
    NotifyError(#[from] notify::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, SyncError>;
