//! File synchronization for remote DevDock workspaces.
//!
//! This crate provides real-time file synchronization between local and remote
//! workspaces using rsync and file watching.

pub mod daemon;
pub mod force;
pub mod git;
pub mod health;
pub mod policy;
pub mod rsync;
pub mod stats;
pub mod watcher;

pub use daemon::SyncDaemon;
pub use git::{list_all_files, list_git_modified_files, list_git_tracked_files};
pub use health::HealthMonitor;
pub use policy::{
    universal_matcher, BenignRuleV1, PathClassification, PolicyError, RepositorySyncPolicyV1,
    RuleSource, SyncMatcher, SYNC_POLICY_SCHEMA_VERSION, UNIVERSAL_BASELINE_VERSION,
};
pub use rsync::RsyncClient;
pub use stats::{StatsUpdate, SyncStatsTracker};
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
