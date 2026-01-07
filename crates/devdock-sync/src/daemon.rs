//! Sync daemon for background file synchronization.

use crate::{git, health::HealthMonitor, rsync::RsyncClient, watcher::FileWatcher, Result};
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
        let remote_path = PathBuf::from("/home/workspace/scio");
        
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
        eprintln!("[SYNC] Starting initial_sync, full_mode={}", self.full_mode);
        if self.full_mode {
            info!("Starting initial sync in full mode");
            eprintln!("[SYNC] Calling rsync_client.sync_all()");
            self.rsync_client.sync_all()?;
        } else {
            info!("Listing git-tracked files for initial sync");
            eprintln!("[SYNC] Listing git-tracked files...");
            let files = git::list_git_tracked_files(&self.repo_path)?;
            info!("Found {} git-tracked files to sync", files.len());
            eprintln!("[SYNC] Found {} git-tracked files", files.len());
            
            // Sync in batches to avoid overwhelming rsync
            // Python uses MAX_BATCH_SIZE = 5, we use slightly larger for efficiency
            const BATCH_SIZE: usize = 10;
            let total_batches = (files.len() + BATCH_SIZE - 1) / BATCH_SIZE;
            eprintln!("[SYNC] Will sync in {} batches of up to {} files", total_batches, BATCH_SIZE);
            
            for (batch_num, chunk) in files.chunks(BATCH_SIZE).enumerate() {
                info!("Syncing batch {}/{} ({} files)", batch_num + 1, total_batches, chunk.len());
                eprintln!("[SYNC] Syncing batch {}/{} ({} files)", batch_num + 1, total_batches, chunk.len());
                
                match self.rsync_client.sync_files(chunk, false) {
                    Ok(_) => {
                        eprintln!("[SYNC] Batch {}/{} completed", batch_num + 1, total_batches);
                    }
                    Err(e) => {
                        eprintln!("[SYNC] ERROR in batch {}/{}: {}", batch_num + 1, total_batches, e);
                        return Err(e);
                    }
                }
            }
            info!("Initial sync completed successfully");
            eprintln!("[SYNC] Initial sync completed!");
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
        
        // Also create a log file for this sync daemon
        let log_file = pid_dir.join(format!("{}.log", self.devdock_id));
        let _ = fs::write(&log_file, format!("Sync daemon started for {}\nWorkspace: {}\nRepo: {}\n", 
            self.devdock_id, self.workspace_name, self.repo_path.display()));
        info!("Log file created: {}", log_file.display());
        
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

/// Start sync daemon in foreground (for debugging).
pub fn start_daemon_foreground(
    devdock_id: &str,
    workspace_name: &str,
    repo_path: &Path,
    full_mode: bool,
) -> Result<()> {
    info!("Starting sync daemon in FOREGROUND");
    info!("DevDock ID: {}", devdock_id);
    info!("Workspace: {}", workspace_name);
    info!("Repo path: {}", repo_path.display());
    info!("Full mode: {}", full_mode);
    
    let mut daemon = SyncDaemon::new(
        devdock_id.to_string(),
        workspace_name.to_string(),
        repo_path.to_path_buf(),
        full_mode,
    )?;
    
    daemon.start()
}

/// Start sync daemon in background.
pub fn start_daemon(
    devdock_id: &str,
    workspace_name: &str,
    repo_path: &Path,
    full_mode: bool,
) -> Result<u32> {
    info!("Starting sync daemon in background");
    info!("DevDock ID: {}", devdock_id);
    info!("Workspace: {}", workspace_name);
    info!("Repo path: {}", repo_path.display());
    info!("Full mode: {}", full_mode);
    
    // For now, we'll start it in a separate thread
    // In production, this should be a proper daemon process
    let devdock_id = devdock_id.to_string();
    let workspace_name = workspace_name.to_string();
    let repo_path = repo_path.to_path_buf();
    
    std::thread::spawn(move || {
        // Set up panic hook to log panics
        let default_panic = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            eprintln!("SYNC DAEMON PANIC: {:?}", panic_info);
            default_panic(panic_info);
        }));
        
        // Get log file path for error logging
        let pid_dir = SyncDaemon::get_pid_dir();
        let log_file = pid_dir.join(format!("{}.log", devdock_id));
        
        let write_log = |msg: &str| {
            use std::fs::OpenOptions;
            use std::io::Write;
            eprintln!("SYNC DAEMON: {}", msg);  // Also log to stderr for debugging
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&log_file) {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let _ = writeln!(f, "[{}] {}", timestamp, msg);
            }
        };
        
        write_log(&format!("Sync daemon thread started for {}", devdock_id));
        
        let mut daemon = match SyncDaemon::new(devdock_id.clone(), workspace_name.clone(), repo_path.clone(), full_mode) {
            Ok(d) => {
                write_log(&format!("Sync daemon created successfully for {}", devdock_id));
                d
            },
            Err(e) => {
                write_log(&format!("ERROR: Failed to create sync daemon: {}", e));
                return;
            }
        };
        
        if let Err(e) = daemon.start() {
            write_log(&format!("ERROR: Sync daemon error: {}", e));
        } else {
            write_log("Sync daemon stopped normally");
        }
    });
    
    // Give the thread a moment to start
    std::thread::sleep(std::time::Duration::from_millis(500));
    
    Ok(std::process::id())
}

/// Stop sync daemon and clean up all related files.
pub fn stop_daemon(devdock_id: &str) -> Result<()> {
    cleanup_sync_files(devdock_id)
}

/// Clean up all sync files for a specific devdock instance.
pub fn cleanup_sync_files(devdock_id: &str) -> Result<()> {
    let pid_dir = SyncDaemon::get_pid_dir();
    let pid_file = pid_dir.join(format!("{}.pid", devdock_id));
    
    if pid_file.exists() {
        let pid_str = fs::read_to_string(&pid_file)?;
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
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
        }
    }
    
    // Clean up all sync files for this devdock_id
    let sync_files = vec![
        format!("{}.pid", devdock_id),
        format!("{}.excludes", devdock_id),
        format!("{}.hb", devdock_id),
        format!("{}.stats", devdock_id),
        format!("{}.log", devdock_id),
        format!("{}_portforward.pid", devdock_id),
    ];
    
    for file_name in sync_files {
        let file_path = pid_dir.join(&file_name);
        if file_path.exists() {
            let _ = fs::remove_file(&file_path);
        }
    }
    
    info!("Cleaned up sync files for {}", devdock_id);
    Ok(())
}

