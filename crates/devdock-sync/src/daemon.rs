//! Sync daemon for background file synchronization.

use crate::{git, health::HealthMonitor, rsync::RsyncClient, watcher::FileWatcher, Result, SyncError};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, error, info};

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
        eprintln!("[SYNC] About to call initial_sync()");
        self.initial_sync()?;
        eprintln!("[SYNC] initial_sync() completed successfully");
        
        // Start watching for changes - watch specific monitored directories, not entire repo
        eprintln!("[SYNC] Setting up file watcher for monitored directories...");
        
        // Only watch directories that exist
        let monitored_dirs = vec![
            "go", "python_scio", "typescript", "java", "com/askscio/proto",
            "config", "javatests", "openapi", "devdock",
        ];
        
        let mut watched_count = 0;
        for dir in &monitored_dirs {
            let dir_path = self.repo_path.join(dir);
            if dir_path.exists() {
                eprintln!("[SYNC] Watching directory: {}", dir);
                self.file_watcher.watch(&dir_path)?;
                watched_count += 1;
            } else {
                eprintln!("[SYNC] Skipping non-existent directory: {}", dir);
            }
        }
        
        eprintln!("[SYNC] File watcher setup complete, watching {} directories", watched_count);
        
        // Main event loop
        info!("File sync daemon running, watching for changes...");
        eprintln!("[SYNC] Entering main event loop...");
        
        let mut iteration = 0;
        loop {
            iteration += 1;
            if iteration % 100 == 0 {
                eprintln!("[SYNC] Event loop iteration {}, daemon is alive", iteration);
            }
            
            // Check health
            if !self.health_monitor.is_healthy() {
                error!("Sync daemon unhealthy, exiting");
                eprintln!("[SYNC] ❌ Health check failed!");
                if let Some(lag) = self.health_monitor.sync_lag() {
                    eprintln!("[SYNC] Time since last sync: {:?}", lag);
                } else {
                    eprintln!("[SYNC] No successful syncs yet");
                }
                eprintln!("[SYNC] Daemon exiting due to health check failure");
                break;
            }
            
            // Get changed files (with timeout)
            match self.file_watcher.get_changed_files(Duration::from_secs(10)) {
                Ok(files) => {
                    if !files.is_empty() {
                        eprintln!("[SYNC] Detected {} file changes", files.len());
                        for (i, file) in files.iter().enumerate().take(10) {
                            eprintln!("[SYNC]   Change {}: {:?}", i + 1, file);
                        }
                        if files.len() > 10 {
                            eprintln!("[SYNC]   ... and {} more", files.len() - 10);
                        }
                        
                        // Filter files based on mode
                        let files_to_sync = self.filter_files(&files)?;
                        
                        eprintln!("[SYNC] After filtering: {} files to sync", files_to_sync.len());
                        
                        if !files_to_sync.is_empty() {
                            for file in files_to_sync.iter().take(5) {
                                eprintln!("[SYNC]   Syncing: {:?}", file);
                            }
                            
                            // Sync changed files
                            match self.rsync_client.sync_files(&files_to_sync, self.full_mode) {
                                Ok(_) => {
                                    self.health_monitor.record_success();
                                    info!("Synced {} files", files_to_sync.len());
                                    eprintln!("[SYNC] ✅ Successfully synced {} files", files_to_sync.len());
                                }
                                Err(e) => {
                                    self.health_monitor.record_failure();
                                    error!("Sync failed: {}", e);
                                    eprintln!("[SYNC] ❌ Sync failed: {}", e);
                                }
                            }
                        } else {
                            eprintln!("[SYNC] All detected files were filtered out (not in monitored dirs or wrong extension)");
                        }
                    }
                }
                Err(e) => {
                    // Watcher errors are usually just timeouts (no changes), not real failures
                    // Only log as debug, don't count as health failure
                    debug!("Watcher timeout or error: {}", e);
                    eprintln!("[SYNC] Watcher timeout (no changes): {}", e);
                    // Don't record as failure - this is normal when there are no file changes
                }
            }
        }
        
        self.cleanup()?;
        Ok(())
    }
    
    /// Perform initial sync of modified files only (fast startup).
    /// This matches Python's strategy: sync only git-modified files, not all tracked files.
    fn initial_sync(&mut self) -> Result<()> {
        eprintln!("[SYNC] Starting initial_sync, full_mode={}", self.full_mode);
        
        if self.full_mode {
            info!("Starting initial sync in full mode");
            eprintln!("[SYNC] Calling rsync_client.sync_all()");
            self.rsync_client.sync_all()?;
        } else {
            // FAST STARTUP: Only sync git-modified files (changed since origin/master)
            // This dramatically reduces startup time from minutes to seconds
            info!("Syncing git-modified files only (fast startup)");
            eprintln!("[SYNC] Using FAST startup: syncing only git-modified files...");
            
            let files = git::list_git_modified_files(&self.repo_path)?;
            
            if files.is_empty() {
                info!("No modified files to sync - you're up to date!");
                eprintln!("[SYNC] No modified files found - workspace is up to date");
            } else {
                info!("Found {} modified files to sync", files.len());
                eprintln!("[SYNC] Found {} modified files to sync", files.len());
                
                // Sync modified files in small batches
                const BATCH_SIZE: usize = 10;
                let total_batches = files.len().div_ceil(BATCH_SIZE);
                
                for (batch_num, chunk) in files.chunks(BATCH_SIZE).enumerate() {
                    info!("Syncing batch {}/{} ({} files)", batch_num + 1, total_batches, chunk.len());
                    eprintln!("[SYNC] Syncing batch {}/{} ({} files)", batch_num + 1, total_batches, chunk.len());
                    
                    match self.rsync_client.sync_files(chunk, false) {
                        Ok(_) => {
                            eprintln!("[SYNC] Batch {}/{} completed successfully", batch_num + 1, total_batches);
                        }
                        Err(e) => {
                            eprintln!("[SYNC] ERROR in batch {}/{}: {}", batch_num + 1, total_batches, e);
                            return Err(e);
                        }
                    }
                    eprintln!("[SYNC] After batch {}/{} sync", batch_num + 1, total_batches);
                }
                info!("Modified files sync completed successfully");
                eprintln!("[SYNC] All batches completed!");
            }
            
            eprintln!("[SYNC] About to record success in health monitor");
            // NOTE: Full baseline sync will happen later via file watcher
            // This approach prioritizes fast startup over completeness
        }
        
        eprintln!("[SYNC] Recording success...");
        self.health_monitor.record_success();
        eprintln!("[SYNC] Health monitor updated, returning Ok");
        Ok(())
    }
    
    /// Filter files based on sync mode and patterns.
    fn filter_files(&self, files: &[PathBuf]) -> Result<Vec<PathBuf>> {
        let mut filtered = Vec::new();
        
        for file in files {
            // Convert absolute path to relative path
            let relative_path = if file.starts_with(&self.repo_path) {
                file.strip_prefix(&self.repo_path)
                    .unwrap_or(file.as_path())
                    .to_path_buf()
            } else {
                file.clone()
            };
            
            eprintln!("[SYNC-FILTER] Checking file: {:?}", relative_path);
            
            // Check if file matches sync patterns (monitored dirs + extensions)
            if !git::should_sync_file_by_pattern(&relative_path) {
                eprintln!("[SYNC-FILTER]   ❌ Filtered out: not in monitored dirs or wrong extension");
                continue;
            }
            
            // In git-tracked mode, also check if file is tracked
            if !self.full_mode
                && !git::should_sync_file(&self.repo_path, file, false)? {
                    eprintln!("[SYNC-FILTER]   ❌ Filtered out: not git-tracked");
                    continue;
                }
            
            eprintln!("[SYNC-FILTER]   ✅ Will sync: {:?}", relative_path);
            filtered.push(relative_path);
        }
        
        Ok(filtered)
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

/// Start sync daemon as a separate background process.
pub fn start_daemon(
    devdock_id: &str,
    workspace_name: &str,
    repo_path: &Path,
    full_mode: bool,
) -> Result<u32> {
    use std::fs::OpenOptions;
    use std::process::Command;
    
    info!("Starting sync daemon as background process");
    info!("DevDock ID: {}", devdock_id);
    info!("Workspace: {}", workspace_name);
    info!("Repo path: {}", repo_path.display());
    info!("Full mode: {}", full_mode);
    
    // Get the current executable path
    let exe_path = std::env::current_exe()
        .map_err(|e| SyncError::DaemonError(format!("Failed to get current exe: {}", e)))?;
    
    // Get PID directory and ensure it exists
    let pid_dir = SyncDaemon::get_pid_dir();
    fs::create_dir_all(&pid_dir)?;
    
    // Create log file for daemon output
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(pid_dir.join(format!("{}.log", devdock_id)))
        .map_err(|e| SyncError::DaemonError(format!("Failed to open log file: {}", e)))?;
    
    // Spawn daemon as a separate process using a hidden subcommand
    let mut cmd = Command::new(exe_path);
    cmd.arg("__sync_daemon__")  // Hidden internal command
        .arg(devdock_id)
        .arg(workspace_name)
        .arg(repo_path)
        .arg(if full_mode { "true" } else { "false" });
    
    // Redirect stdin/stdout/stderr to log file
    cmd.stdin(std::process::Stdio::null())
        .stdout(log_file.try_clone().map(std::process::Stdio::from).unwrap_or(std::process::Stdio::null()))
        .stderr(std::process::Stdio::from(log_file));
    
    let child = cmd.spawn()
        .map_err(|e| SyncError::DaemonError(format!("Failed to spawn daemon process: {}", e)))?;
    
    let pid = child.id();
    // Debug output now goes to log file via stderr redirect, not to console
    
    // Don't wait for child - let it run independently
    // The child will write its own PID file
    
    std::thread::sleep(std::time::Duration::from_millis(500));
    Ok(pid)
}

/// Run the sync daemon (called by hidden __sync_daemon__ command).
pub fn run_daemon_process(
    devdock_id: &str,
    workspace_name: &str,
    repo_path: &Path,
    full_mode: bool,
) -> Result<()> {
    // This function runs in the daemonized child process
    let devdock_id = devdock_id.to_string();
    let workspace_name = workspace_name.to_string();
    let repo_path = repo_path.to_path_buf();
    
    // Print startup info (will be captured in log file via stderr redirect from parent)
    eprintln!("\n=== Sync daemon process started ===");
    eprintln!("DevDock ID: {}", devdock_id);
    eprintln!("Workspace: {}", workspace_name);
    eprintln!("Repo: {}", repo_path.display());
    eprintln!("Time: {:?}\n", std::time::SystemTime::now());
    
    let mut daemon = match SyncDaemon::new(devdock_id.clone(), workspace_name.clone(), repo_path.clone(), full_mode) {
        Ok(d) => {
            eprintln!("Sync daemon created successfully");
            d
        },
        Err(e) => {
            eprintln!("ERROR: Failed to create sync daemon: {}", e);
            return Err(e);
        }
    };
    
    eprintln!("Starting daemon main loop...");
    if let Err(e) = daemon.start() {
        eprintln!("ERROR: Sync daemon error: {}", e);
        Err(e)
    } else {
        eprintln!("Sync daemon stopped normally");
        Ok(())
    }
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

