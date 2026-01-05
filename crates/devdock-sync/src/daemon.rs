//! Sync daemon for background file synchronization.

use crate::{git, health::HealthMonitor, rsync::RsyncClient, watcher::FileWatcher, Result, SyncError};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{error, info, warn};

/// Sync daemon state.
pub struct SyncDaemon {
    devdock_id: String,
    workspace_name: String,
    rsync_client: RsyncClient,
    file_watcher: FileWatcher,
    health_monitor: HealthMonitor,
    repo_path: PathBuf,
    full_mode: bool,
}

impl SyncDaemon {
    /// Create a new sync daemon.
    pub fn new(
        devdock_id: String,
        workspace_name: String,
        repo_path: PathBuf,
        full_mode: bool,
    ) -> Result<Self> {
        let remote_path = PathBuf::from("~/scio");
        
        let rsync_client = RsyncClient::new(
            workspace_name.clone(),
            repo_path.clone(),
            remote_path,
        );
        
        let file_watcher = FileWatcher::new()?;
        let health_monitor = HealthMonitor::new(workspace_name.clone());
        
        Ok(Self {
            devdock_id,
            workspace_name,
            rsync_client,
            file_watcher,
            health_monitor,
            repo_path,
            full_mode,
        })
    }
    
    /// Start the sync daemon.
    pub fn start(&mut self) -> Result<()> {
        info!("Starting sync daemon for {}", self.workspace_name);
        
        // Write PID file
        self.write_pid_file()?;
        
        // Perform initial sync
        info!("Performing initial sync (mode: {})", if self.full_mode { "full" } else { "git-tracked" });
        self.initial_sync()?;
        
        // Start watching for changes
        self.file_watcher.watch(&self.repo_path)?;
        
        // Main event loop
        info!("File sync daemon running, watching for changes...");
        loop {
            // Check health
            if !self.health_monitor.is_healthy() {
                error!("Sync daemon unhealthy, exiting");
                break;
            }
            
            // Get changed files (with timeout)
            match self.file_watcher.get_changed_files(Duration::from_secs(10)) {
                Ok(files) => {
                    if !files.is_empty() {
                        // Filter files based on mode
                        let files_to_sync = self.filter_files(&files)?;
                        
                        if !files_to_sync.is_empty() {
                            // Sync changed files
                            match self.rsync_client.sync_files(&files_to_sync, self.full_mode) {
                                Ok(_) => {
                                    self.health_monitor.record_success();
                                    info!("Synced {} files", files_to_sync.len());
                                }
                                Err(e) => {
                                    self.health_monitor.record_failure();
                                    error!("Sync failed: {}", e);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Watcher error: {}", e);
                    self.health_monitor.record_failure();
                }
            }
        }
        
        self.cleanup()?;
        Ok(())
    }
    
    /// Perform initial sync of all relevant files.
    fn initial_sync(&mut self) -> Result<()> {
        if self.full_mode {
            self.rsync_client.sync_all()?;
        } else {
            let files = git::list_git_tracked_files(&self.repo_path)?;
            self.rsync_client.sync_files(&files, false)?;
        }
        
        self.health_monitor.record_success();
        Ok(())
    }
    
    /// Filter files based on sync mode.
    fn filter_files(&self, files: &[PathBuf]) -> Result<Vec<PathBuf>> {
        if self.full_mode {
            Ok(files.to_vec())
        } else {
            // Only include git-tracked files
            let mut filtered = Vec::new();
            for file in files {
                if git::should_sync_file(&self.repo_path, file, false)? {
                    filtered.push(file.clone());
                }
            }
            Ok(filtered)
        }
    }
    
    /// Write PID file.
    fn write_pid_file(&self) -> Result<()> {
        let pid_dir = Self::get_pid_dir();
        fs::create_dir_all(&pid_dir)?;
        
        let pid_file = pid_dir.join(format!("{}.pid", self.devdock_id));
        fs::write(&pid_file, std::process::id().to_string())?;
        
        Ok(())
    }
    
    /// Clean up PID file and other resources.
    fn cleanup(&self) -> Result<()> {
        let pid_dir = Self::get_pid_dir();
        let pid_file = pid_dir.join(format!("{}.pid", self.devdock_id));
        
        if pid_file.exists() {
            let _ = fs::remove_file(&pid_file);
        }
        
        Ok(())
    }
    
    /// Get PID directory.
    fn get_pid_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".devdock/sync")
    }
}

impl Drop for SyncDaemon {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Start sync daemon in background.
pub fn start_daemon(
    devdock_id: &str,
    workspace_name: &str,
    repo_path: &Path,
    full_mode: bool,
) -> Result<u32> {
    info!("Starting sync daemon in background");
    
    // For now, we'll start it in a separate thread
    // In production, this should be a proper daemon process
    let devdock_id = devdock_id.to_string();
    let workspace_name = workspace_name.to_string();
    let repo_path = repo_path.to_path_buf();
    
    std::thread::spawn(move || {
        let mut daemon = match SyncDaemon::new(devdock_id, workspace_name, repo_path, full_mode) {
            Ok(d) => d,
            Err(e) => {
                error!("Failed to create sync daemon: {}", e);
                return;
            }
        };
        
        if let Err(e) = daemon.start() {
            error!("Sync daemon error: {}", e);
        }
    });
    
    Ok(std::process::id())
}

/// Stop sync daemon.
pub fn stop_daemon(devdock_id: &str) -> Result<()> {
    let pid_dir = SyncDaemon::get_pid_dir();
    let pid_file = pid_dir.join(format!("{}.pid", devdock_id));
    
    if !pid_file.exists() {
        return Ok(());
    }
    
    let pid_str = fs::read_to_string(&pid_file)?;
    let pid: u32 = pid_str.trim().parse()
        .map_err(|_| SyncError::DaemonError("Invalid PID".to_string()))?;
    
    // Kill process
    #[cfg(unix)]
    {
        use std::process::Command;
        let _ = Command::new("kill").arg(pid.to_string()).output();
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
    
    // Remove PID file
    let _ = fs::remove_file(&pid_file);
    
    Ok(())
}

