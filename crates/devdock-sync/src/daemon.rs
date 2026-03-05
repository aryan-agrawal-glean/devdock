//! Sync daemon for background file synchronization.

use crate::{
    git,
    health::HealthMonitor,
    rsync::RsyncClient,
    stats::{StatsUpdate, SyncStatsTracker},
    watcher::FileWatcher,
    Result, SyncError,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use tracing::{debug, error, info};

/// Sync daemon state.
pub struct SyncDaemon {
    devdock_id: String,
    workspace_name: String,
    rsync_client: RsyncClient,
    file_watcher: FileWatcher,
    health_monitor: HealthMonitor,
    stats_tracker: SyncStatsTracker,
    repo_path: PathBuf,
    full_mode: bool,
    branch_mode: bool,
    force_patterns: Vec<String>,
    force_patterns_mtime: Option<SystemTime>,
}

impl SyncDaemon {
    /// Create a new sync daemon.
    pub fn new(
        devdock_id: String,
        workspace_name: String,
        repo_path: PathBuf,
        full_mode: bool,
        branch_mode: bool,
    ) -> Result<Self> {
        let remote_path = PathBuf::from("/home/workspace/scio");

        let rsync_client = RsyncClient::new(workspace_name.clone(), repo_path.clone(), remote_path);

        let file_watcher = FileWatcher::new()?;
        let health_monitor = HealthMonitor::new(workspace_name.clone());
        let stats_tracker = SyncStatsTracker::new(&devdock_id)?;

        // Load persisted force-watch patterns
        let force_patterns = crate::force::load_force_patterns(&devdock_id)
            .map(|fp| fp.patterns)
            .unwrap_or_default();
        let force_patterns_mtime = Self::read_force_patterns_mtime(&devdock_id);

        if !force_patterns.is_empty() {
            info!("Loaded {} force-watch patterns", force_patterns.len());
        }

        Ok(Self {
            devdock_id,
            workspace_name,
            rsync_client,
            file_watcher,
            health_monitor,
            stats_tracker,
            repo_path,
            full_mode,
            branch_mode,
            force_patterns,
            force_patterns_mtime,
        })
    }

    /// Start the sync daemon.
    pub fn start(&mut self) -> Result<()> {
        info!("Starting sync daemon for {}", self.workspace_name);

        // Write PID file
        self.write_pid_file()?;

        // Perform initial sync
        info!(
            "Performing initial sync (mode: {})",
            if self.full_mode {
                "full"
            } else if self.branch_mode {
                "branch"
            } else {
                "git-tracked"
            }
        );
        debug!("About to call initial_sync()");
        self.initial_sync()?;
        debug!("initial_sync() completed successfully");

        // Start watching for changes - watch specific monitored directories, not entire repo
        debug!("Setting up file watcher for monitored directories...");

        // Use the canonical MONITORED_DIRS from git.rs (strip trailing slashes for path joining)
        let monitored_dirs: Vec<&str> =
            git::MONITORED_DIRS.iter().map(|d| d.trim_end_matches('/')).collect();

        let mut watched_count = 0;
        for dir in &monitored_dirs {
            let dir_path = self.repo_path.join(dir);
            if dir_path.exists() {
                debug!("Watching directory: {dir}");
                self.file_watcher.watch(&dir_path)?;
                watched_count += 1;
            } else {
                debug!("Skipping non-existent directory: {dir}");
            }
        }

        info!("File watcher setup complete, watching {watched_count} directories");

        // Watch directories for any force-watch patterns
        self.watch_force_pattern_dirs();

        // Main event loop
        info!("File sync daemon running, watching for changes...");
        debug!("Entering main event loop...");

        let mut iteration = 0;
        loop {
            iteration += 1;
            if iteration % 100 == 0 {
                debug!("Event loop iteration {iteration}, daemon is alive");
            }

            // Check for updated force-watch patterns (file-based polling)
            self.reload_force_patterns_if_changed();

            // Check health
            if !self.health_monitor.is_healthy() {
                error!("Health check failed!");
                if let Some(lag) = self.health_monitor.sync_lag() {
                    error!("Time since last sync: {lag:?}");
                } else {
                    error!("No successful syncs yet");
                }
                error!("Daemon exiting due to health check failure");
                break;
            }

            // Get changed files (with timeout)
            match self.file_watcher.get_changed_files(Duration::from_secs(10)) {
                Ok(files) => {
                    if !files.is_empty() {
                        debug!("Detected {} file changes", files.len());
                        for (i, file) in files.iter().enumerate().take(10) {
                            debug!("  Change {}: {:?}", i + 1, file);
                        }
                        if files.len() > 10 {
                            debug!("  ... and {} more", files.len() - 10);
                        }

                        // Filter files based on mode
                        let files_to_sync = self.filter_files(&files)?;

                        debug!("After filtering: {} files to sync", files_to_sync.len());

                        if files_to_sync.is_empty() {
                            debug!("All detected files were filtered out (not in monitored dirs or wrong extension)");
                        } else {
                            // Update pending files in stats (for status visibility)
                            self.stats_tracker.set_pending_files(files_to_sync.clone());
                            for file in files_to_sync.iter().take(5) {
                                debug!("  Syncing: {file:?}");
                            }

                            // Mark files as currently syncing
                            self.stats_tracker.start_syncing(&files_to_sync);

                            // Sync changed files (measure duration)
                            let sync_start = Instant::now();
                            match self.rsync_client.sync_files(&files_to_sync, self.full_mode) {
                                Ok(()) => {
                                    let duration = sync_start.elapsed();

                                    // Estimate bytes (rough approximation)
                                    let bytes = self.estimate_file_sizes(&files_to_sync);

                                    // Clear syncing state
                                    self.stats_tracker.finish_syncing();

                                    // Record success in health monitor and stats
                                    self.health_monitor.record_success();
                                    self.stats_tracker.record_success(files_to_sync.len(), bytes);

                                    // Send detailed update to background writer
                                    self.stats_tracker.send_update(StatsUpdate {
                                        files: files_to_sync.clone(),
                                        bytes,
                                        duration_ms: duration.as_millis() as u64,
                                        success: true,
                                        error: None,
                                    });

                                    info!(
                                        "Successfully synced {} files in {}ms",
                                        files_to_sync.len(),
                                        duration.as_millis()
                                    );
                                }
                                Err(e) => {
                                    let duration = sync_start.elapsed();

                                    // Clear syncing state
                                    self.stats_tracker.finish_syncing();

                                    // Record failure
                                    self.health_monitor.record_failure();
                                    self.stats_tracker.record_failure();

                                    // Send error update to background writer
                                    self.stats_tracker.send_update(StatsUpdate {
                                        files: files_to_sync.clone(),
                                        bytes: 0,
                                        duration_ms: duration.as_millis() as u64,
                                        success: false,
                                        error: Some(e.to_string()),
                                    });

                                    error!("Sync failed: {e}");
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    // Watcher errors are usually just timeouts (no changes), not real failures
                    // Only log as debug, don't count as health failure
                    debug!("Watcher timeout (no changes): {e}");
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
        debug!("Starting initial_sync, full_mode={}", self.full_mode);

        if self.full_mode {
            info!("Starting initial sync in full mode");
            debug!("Calling rsync_client.sync_all()");
            self.rsync_client.sync_all()?;
        } else {
            let files = if self.branch_mode {
                // BRANCH MODE: Sync all git-tracked files + untracked
                // Used when --branch flag is specified to ensure complete branch sync
                info!("Syncing all git-tracked and untracked files (branch mode)");
                git::list_git_tracked_and_untracked_files(&self.repo_path)?
            } else {
                // FAST STARTUP: Only sync git-modified files (changed since origin/master)
                // This dramatically reduces startup time from minutes to seconds
                info!("Syncing git-modified files only (fast startup)");
                git::list_git_modified_files(&self.repo_path)?
            };

            if files.is_empty() {
                info!("No modified files to sync - workspace is up to date");
            } else {
                info!("Found {} modified files to sync", files.len());

                // Sync modified files in small batches
                const BATCH_SIZE: usize = 10;
                let total_batches = files.len().div_ceil(BATCH_SIZE);

                for (batch_num, chunk) in files.chunks(BATCH_SIZE).enumerate() {
                    info!(
                        "Syncing batch {}/{} ({} files)",
                        batch_num + 1,
                        total_batches,
                        chunk.len()
                    );

                    match self.rsync_client.sync_files(chunk, false) {
                        Ok(()) => {
                            debug!(
                                "Batch {}/{} completed successfully",
                                batch_num + 1,
                                total_batches
                            );
                        }
                        Err(e) => {
                            error!("Error in batch {}/{}: {}", batch_num + 1, total_batches, e);
                            return Err(e);
                        }
                    }
                }
                info!("Modified files sync completed successfully");
            }

            debug!("About to record success in health monitor");
            // NOTE: Full baseline sync will happen later via file watcher
            // This approach prioritizes fast startup over completeness
        }

        debug!("Recording success...");
        self.health_monitor.record_success();
        debug!("Health monitor updated, returning Ok");
        Ok(())
    }

    /// Estimate total size of files (rough approximation for stats)
    fn estimate_file_sizes(&self, files: &[PathBuf]) -> u64 {
        files
            .iter()
            .filter_map(|file| {
                let full_path =
                    if file.is_absolute() { file.clone() } else { self.repo_path.join(file) };
                std::fs::metadata(&full_path).ok().map(|m| m.len())
            })
            .sum()
    }

    /// Filter files based on sync mode and patterns.
    fn filter_files(&self, files: &[PathBuf]) -> Result<Vec<PathBuf>> {
        let mut filtered = Vec::new();

        for file in files {
            // Convert absolute path to relative path
            let relative_path = if file.starts_with(&self.repo_path) {
                file.strip_prefix(&self.repo_path).unwrap_or(file.as_path()).to_path_buf()
            } else {
                file.clone()
            };

            debug!("Checking file: {relative_path:?}");

            // Check force patterns first - bypass all other filters
            if !self.force_patterns.is_empty()
                && crate::force::matches_force_pattern(&relative_path, &self.force_patterns)
            {
                debug!("✅ Force pattern match, bypassing filters: {relative_path:?}");
                filtered.push(relative_path);
                continue;
            }

            // Check if file matches sync patterns (monitored dirs + extensions)
            if !git::should_sync_file_by_pattern(&relative_path) {
                debug!("❌ Filtered out: not in monitored dirs or wrong extension");
                continue;
            }

            // In git-tracked mode, also check if file is tracked
            if !self.full_mode && !git::should_sync_file(&self.repo_path, file, false)? {
                debug!("❌ Filtered out: not git-tracked");
                continue;
            }

            debug!("✅ Will sync: {relative_path:?}");
            filtered.push(relative_path);
        }

        Ok(filtered)
    }

    /// Watch directories implied by force-watch patterns.
    fn watch_force_pattern_dirs(&mut self) {
        for pattern in &self.force_patterns {
            // Extract the directory prefix before any glob metacharacter
            let dir_part = pattern.split(['*', '?', '[']).next().unwrap_or("");

            let dir = if dir_part.ends_with('/') {
                dir_part.trim_end_matches('/')
            } else if let Some(pos) = dir_part.rfind('/') {
                &dir_part[..pos]
            } else {
                continue;
            };

            if dir.is_empty() {
                continue;
            }

            let dir_path = self.repo_path.join(dir);
            if dir_path.exists() && dir_path.is_dir() {
                if let Err(e) = self.file_watcher.watch(&dir_path) {
                    debug!("Failed to watch force pattern dir {}: {}", dir, e);
                } else {
                    info!("Watching force-pattern directory: {}", dir);
                }
            }
        }
    }

    /// Reload force-watch patterns if the file has been modified.
    fn reload_force_patterns_if_changed(&mut self) {
        let patterns_path = crate::force::force_patterns_path(&self.devdock_id);
        if let Ok(meta) = fs::metadata(&patterns_path) {
            if let Ok(modified) = meta.modified() {
                if self.force_patterns_mtime.map_or(true, |m| modified > m) {
                    info!("Force patterns file changed, reloading...");
                    if let Ok(fp) = crate::force::load_force_patterns(&self.devdock_id) {
                        self.force_patterns = fp.patterns;
                        self.watch_force_pattern_dirs();
                    }
                    self.force_patterns_mtime = Some(modified);
                }
            }
        }
    }

    /// Read the modification time of the force patterns file.
    fn read_force_patterns_mtime(devdock_id: &str) -> Option<SystemTime> {
        let path = crate::force::force_patterns_path(devdock_id);
        fs::metadata(&path).ok().and_then(|m| m.modified().ok())
    }

    /// Write PID file.
    fn write_pid_file(&self) -> Result<()> {
        let pid_dir = Self::get_pid_dir();
        fs::create_dir_all(&pid_dir)?;

        let pid_file = pid_dir.join(format!("{}.pid", self.devdock_id));
        fs::write(&pid_file, std::process::id().to_string())?;

        // Also create a log file for this sync daemon
        let log_file = pid_dir.join(format!("{}.log", self.devdock_id));
        let _ = fs::write(
            &log_file,
            format!(
                "Sync daemon started for {}\nWorkspace: {}\nRepo: {}\n",
                self.devdock_id,
                self.workspace_name,
                self.repo_path.display()
            ),
        );
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
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join(".devdock/sync")
    }
}

impl Drop for SyncDaemon {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Map a CLI verbosity count to a tracing filter string.
pub fn verbosity_to_log_level(verbose: u8) -> &'static str {
    match verbose {
        // for sync daemon, minimum log level is INFO
        0 | 1 => "info",
        2 => "debug",
        _ => "trace",
    }
}

/// Start sync daemon in foreground (for debugging).
pub fn start_daemon_foreground(
    devdock_id: &str,
    workspace_name: &str,
    repo_path: &Path,
    full_mode: bool,
    branch_mode: bool,
    log_level: &str,
) -> Result<()> {
    info!("Log level: {}", log_level);
    info!("Starting sync daemon in FOREGROUND");
    info!("DevDock ID: {}", devdock_id);
    info!("Workspace: {}", workspace_name);
    info!("Repo path: {}", repo_path.display());
    info!("Full mode: {}", full_mode);
    info!("Branch mode: {}", branch_mode);

    let mut daemon = SyncDaemon::new(
        devdock_id.to_string(),
        workspace_name.to_string(),
        repo_path.to_path_buf(),
        full_mode,
        branch_mode,
    )?;

    daemon.start()
}

/// Start sync daemon as a separate background process.
pub fn start_daemon(
    devdock_id: &str,
    workspace_name: &str,
    repo_path: &Path,
    full_mode: bool,
    branch_mode: bool,
    log_level: &str,
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
        .map_err(|e| SyncError::DaemonError(format!("Failed to get current exe: {e}")))?;

    // Get PID directory and ensure it exists
    let pid_dir = SyncDaemon::get_pid_dir();
    fs::create_dir_all(&pid_dir)?;

    // Create log file for daemon output
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(pid_dir.join(format!("{devdock_id}.log")))
        .map_err(|e| SyncError::DaemonError(format!("Failed to open log file: {e}")))?;

    // Spawn daemon as a separate process using a hidden subcommand
    let mut cmd = Command::new(exe_path);
    cmd.arg("__sync_daemon__") // Hidden internal command
        .arg(devdock_id)
        .arg(workspace_name)
        .arg(repo_path)
        .arg(if full_mode { "true" } else { "false" })
        .arg(if branch_mode { "true" } else { "false" })
        .arg(log_level);

    // Redirect stdin/stdout/stderr to log file
    cmd.stdin(std::process::Stdio::null())
        .stdout(
            log_file
                .try_clone()
                .map(std::process::Stdio::from)
                .unwrap_or(std::process::Stdio::null()),
        )
        .stderr(std::process::Stdio::from(log_file));

    let child = cmd
        .spawn()
        .map_err(|e| SyncError::DaemonError(format!("Failed to spawn daemon process: {e}")))?;

    let pid = child.id();
    // Tracing output goes to log file via stderr redirect, not to console

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
    branch_mode: bool,
) -> Result<()> {
    // This function runs in the daemonized child process
    let devdock_id = devdock_id.to_string();
    let workspace_name = workspace_name.to_string();
    let repo_path = repo_path.to_path_buf();

    info!("=== Sync daemon process started ===");
    info!("DevDock ID: {devdock_id}");
    info!("Workspace: {workspace_name}");
    info!("Repo: {}", repo_path.display());
    info!("Time: {:?}", std::time::SystemTime::now());

    let mut daemon = match SyncDaemon::new(
        devdock_id.clone(),
        workspace_name.clone(),
        repo_path.clone(),
        full_mode,
        branch_mode,
    ) {
        Ok(d) => {
            info!("Sync daemon created successfully");
            d
        }
        Err(e) => {
            error!("Failed to create sync daemon: {e}");
            return Err(e);
        }
    };

    info!("Starting daemon main loop...");
    if let Err(e) = daemon.start() {
        error!("Sync daemon error: {e}");
        Err(e)
    } else {
        info!("Sync daemon stopped normally");
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
    let pid_file = pid_dir.join(format!("{devdock_id}.pid"));

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
                let _ =
                    Command::new("taskkill").arg("/PID").arg(pid.to_string()).arg("/F").output();
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
