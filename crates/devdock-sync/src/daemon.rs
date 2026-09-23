//! Sync daemon for background file synchronization.

use crate::{
    git,
    health::HealthMonitor,
    rsync::RsyncClient,
    stats::{StatsUpdate, SyncStatsTracker},
    watcher::FileWatcher,
    RepositorySyncPolicyV1, Result, SyncError, SyncMatcher,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use tracing::{debug, error, info};

const SYNC_POLICY_SNAPSHOT_ENV_VAR: &str = "DEVDOCK_SYNC_POLICY_SNAPSHOT";

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
    /// Profile-level patterns remembered so hot-reload can re-merge all three sources
    /// without losing profile/env overlays.
    extra_force_patterns: Vec<String>,
    sync_matcher: SyncMatcher,
}

impl SyncDaemon {
    /// Create a new sync daemon.
    ///
    /// `extra_force_patterns` are merged in on top of the per-instance persisted file
    /// (from `sync force --watch`) and the `DEVDOCK_FORCE_SYNC_PATTERNS` env var. The
    /// caller is expected to supply profile-level patterns here; env-var patterns are
    /// read internally so every entry point picks them up uniformly.
    pub fn new(
        devdock_id: String,
        workspace_name: String,
        repo_path: PathBuf,
        full_mode: bool,
        branch_mode: bool,
        extra_force_patterns: Vec<String>,
    ) -> Result<Self> {
        Self::with_matcher(
            devdock_id,
            workspace_name,
            repo_path,
            full_mode,
            branch_mode,
            extra_force_patterns,
            SyncMatcher::universal(),
        )
    }

    /// Construct a daemon with the exact policy matcher recorded for its session.
    #[allow(clippy::too_many_arguments)]
    pub fn with_matcher(
        devdock_id: String,
        workspace_name: String,
        repo_path: PathBuf,
        full_mode: bool,
        branch_mode: bool,
        extra_force_patterns: Vec<String>,
        sync_matcher: SyncMatcher,
    ) -> Result<Self> {
        let remote_path = PathBuf::from("/home/workspace/scio");

        let rsync_client = RsyncClient::new(workspace_name.clone(), repo_path.clone(), remote_path);

        let file_watcher = FileWatcher::new()?;
        let health_monitor = HealthMonitor::new(workspace_name.clone());
        let stats_tracker = SyncStatsTracker::new(&devdock_id)?;

        // Load persisted force-watch patterns, then layer profile + env on top.
        let persisted = crate::force::load_force_patterns(&devdock_id)
            .map(|fp| fp.patterns)
            .unwrap_or_default();
        let env_patterns = crate::force::patterns_from_env();
        let force_patterns =
            crate::force::merge_patterns(persisted, &extra_force_patterns, &env_patterns);
        let force_patterns_mtime = Self::read_force_patterns_mtime(&devdock_id);

        if !force_patterns.is_empty() {
            info!(
                "Loaded {} force-watch patterns (profile: {}, env: {})",
                force_patterns.len(),
                extra_force_patterns.len(),
                env_patterns.len()
            );
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
            extra_force_patterns,
            sync_matcher,
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
        let watermark = crate::stats::record_initial_sync_success(
            &self.devdock_id,
            &self.workspace_name,
            &self.repo_path,
        )?;
        info!(
            workspace = %watermark.workspace_name,
            repo = %watermark.repo_path,
            completed_at = %watermark.completed_at,
            "Recorded initial sync watermark"
        );

        // Start watching for changes - watch specific monitored directories, not entire repo
        debug!("Setting up file watcher for monitored directories...");

        let monitored_dirs: Vec<String> =
            self.sync_matcher.monitored_roots().map(str::to_string).collect();

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
                git::list_git_tracked_and_untracked_files_with(&self.repo_path, &self.sync_matcher)?
            } else {
                // FAST STARTUP: Only sync git-modified files (changed since origin/master)
                // This dramatically reduces startup time from minutes to seconds
                info!("Syncing git-modified files only (fast startup)");
                git::list_git_modified_files_with(&self.repo_path, &self.sync_matcher)?
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
            if !self.sync_matcher.is_syncable(&relative_path) {
                debug!("❌ Filtered out: not in monitored dirs or wrong extension");
                continue;
            }

            // In git-tracked mode, also check if file is tracked
            if !self.full_mode
                && !git::should_sync_file_with(&self.sync_matcher, &self.repo_path, file, false)?
            {
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

    /// Reload force-watch patterns if the persisted file has been modified, re-merging
    /// with the profile-level patterns and env var on every reload.
    fn reload_force_patterns_if_changed(&mut self) {
        let patterns_path = crate::force::force_patterns_path(&self.devdock_id);
        if let Ok(meta) = fs::metadata(&patterns_path) {
            if let Ok(modified) = meta.modified() {
                if self.force_patterns_mtime.map_or(true, |m| modified > m) {
                    info!("Force patterns file changed, reloading...");
                    if let Ok(fp) = crate::force::load_force_patterns(&self.devdock_id) {
                        let env_patterns = crate::force::patterns_from_env();
                        self.force_patterns = crate::force::merge_patterns(
                            fp.patterns,
                            &self.extra_force_patterns,
                            &env_patterns,
                        );
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
    extra_force_patterns: Vec<String>,
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
        extra_force_patterns,
    )?;

    daemon.start()
}

/// Start sync daemon as a separate background process.
#[allow(clippy::too_many_arguments)]
pub fn start_daemon(
    devdock_id: &str,
    workspace_name: &str,
    repo_path: &Path,
    full_mode: bool,
    branch_mode: bool,
    log_level: &str,
    extra_force_patterns: &[String],
    sync_matcher: &SyncMatcher,
) -> Result<u32> {
    use std::fs::OpenOptions;
    use std::process::Command;

    info!("Starting sync daemon as background process");
    info!("DevDock ID: {}", devdock_id);
    info!("Workspace: {}", workspace_name);
    info!("Repo path: {}", repo_path.display());
    info!("Full mode: {}", full_mode);

    // New child, new attempt: no prior attempt's sync history or error may be
    // used as readiness evidence. The caller first checks for a live owned
    // daemon before invoking this function, so these are exact-ID files only.
    crate::stats::clear_initial_sync_evidence(devdock_id)?;

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

    // Profile-level patterns ride as an env var so the subprocess picks them up without
    // changing the positional arg layout. Any pre-existing `DEVDOCK_FORCE_SYNC_PATTERNS`
    // in the caller's env is preserved and merged back in by the daemon.
    if !extra_force_patterns.is_empty() {
        let existing = std::env::var(crate::force::FORCE_PATTERNS_ENV_VAR).unwrap_or_default();
        let merged = if existing.trim().is_empty() {
            extra_force_patterns.join(",")
        } else {
            format!("{},{}", existing, extra_force_patterns.join(","))
        };
        cmd.env(crate::force::FORCE_PATTERNS_ENV_VAR, merged);
    }

    // Preserve the exact trusted policy used by the parent. Keeping this optional and out of the
    // positional CLI layout lets new children accept old launchers, while an explicit env_remove
    // prevents a stale inherited policy from changing a universal session.
    if let Some(snapshot) = matcher_policy_snapshot(sync_matcher)? {
        cmd.env(SYNC_POLICY_SNAPSHOT_ENV_VAR, snapshot);
    } else {
        cmd.env_remove(SYNC_POLICY_SNAPSHOT_ENV_VAR);
    }

    // Redirect stdin/stdout/stderr to log file
    cmd.stdin(std::process::Stdio::null())
        .stdout(
            log_file
                .try_clone()
                .map(std::process::Stdio::from)
                .unwrap_or(std::process::Stdio::null()),
        )
        .stderr(std::process::Stdio::from(log_file));

    let child = spawn_detached_daemon(&mut cmd)?;

    let pid = child.id();
    // Tracing output goes to log file via stderr redirect, not to console

    // Don't wait for child - let it run independently
    // The child will write its own PID file

    std::thread::sleep(std::time::Duration::from_millis(500));
    Ok(pid)
}

/// Spawn a hidden sync daemon outside the invoking CLI's process session.
/// Noninteractive runners may clean up the command's process group at exit;
/// the daemon's exact PID file and command identity remain its lifecycle and
/// cleanup authority, so it must survive that parent command returning.
fn spawn_detached_daemon(command: &mut std::process::Command) -> Result<std::process::Child> {
    detach_daemon_process(command);
    command
        .spawn()
        .map_err(|error| SyncError::DaemonError(format!("Failed to spawn daemon process: {error}")))
}

#[cfg(unix)]
fn detach_daemon_process(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: `pre_exec` runs in the child after fork and before exec. `setsid`
    // is an async-signal-safe libc call and is the only operation in the hook.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(not(unix))]
fn detach_daemon_process(_command: &mut std::process::Command) {
    // Windows does not use Unix session/process-group teardown. The daemon's
    // PID file and command identity still govern liveness and cleanup.
}

pub fn read_daemon_pid(devdock_id: &str) -> Result<Option<u32>> {
    let pid_file = SyncDaemon::get_pid_dir().join(format!("{devdock_id}.pid"));
    if !pid_file.exists() {
        return Ok(None);
    }

    let pid_str = fs::read_to_string(pid_file)?;
    Ok(parse_pid(&pid_str))
}

pub fn is_daemon_running(devdock_id: &str, pid: u32) -> bool {
    is_process_alive(pid) && process_command_matches_daemon(devdock_id, pid)
}

/// Wait for the daemon's initial scan to complete and prove it with a fresh
/// watermark plus a matching successful history entry. Callers must gate remote
/// runtime launch on this rather than treating a spawned PID as a successful
/// source transfer.
pub fn wait_for_initial_sync_watermark(
    devdock_id: &str,
    workspace_name: &str,
    repo_path: &Path,
    pid: u32,
    timeout: Duration,
) -> Result<crate::stats::InitialSyncWatermark> {
    let started = Instant::now();
    loop {
        if !is_daemon_running(devdock_id, pid) {
            return Err(SyncError::DaemonError(
                "sync daemon stopped before its initial sync watermark".to_string(),
            ));
        }

        if let Some(watermark) = crate::stats::read_initial_sync_watermark(devdock_id)? {
            let expected_repo = repo_path.display().to_string();
            let history = crate::stats::read_history_entries(devdock_id, None)?;
            if watermark.devdock_id == devdock_id
                && watermark.workspace_name == workspace_name
                && watermark.repo_path == expected_repo
                && crate::stats::initial_sync_evidence_matches(&watermark, &history)
                && !crate::stats::has_sync_errors(devdock_id)?
            {
                return Ok(watermark);
            }

            return Err(SyncError::DaemonError(
                "initial sync evidence does not match this workspace attempt".to_string(),
            ));
        }

        if started.elapsed() >= timeout {
            return Err(SyncError::DaemonError(format!(
                "timed out waiting {} seconds for initial sync watermark",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn parse_pid(contents: &str) -> Option<u32> {
    contents.trim().parse::<u32>().ok()
}

fn command_matches_daemon(command: &str, devdock_id: &str) -> bool {
    command.contains("__sync_daemon__") && command.contains(devdock_id)
}

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
fn is_process_alive(pid: u32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}")])
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
        })
}

#[cfg(unix)]
fn process_command_matches_daemon(devdock_id: &str, pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && command_matches_daemon(&String::from_utf8_lossy(&output.stdout), devdock_id)
        })
}

#[cfg(windows)]
fn process_command_matches_daemon(_devdock_id: &str, _pid: u32) -> bool {
    // Windows tasklist does not reliably expose command-line args without PowerShell/CIM.
    // Treat the PID existence check as the liveness signal on Windows for now.
    true
}

/// Run the sync daemon (called by hidden __sync_daemon__ command).
///
/// The subprocess inherits env, so `DEVDOCK_FORCE_SYNC_PATTERNS` carries profile-level patterns
/// and `DEVDOCK_SYNC_POLICY_SNAPSHOT` carries the exact validated repository policy selected by
/// the parent. Both variables are optional for compatibility with older launchers.
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

    let sync_matcher = matcher_from_process_env()?;

    let mut daemon = match SyncDaemon::with_matcher(
        devdock_id.clone(),
        workspace_name.clone(),
        repo_path.clone(),
        full_mode,
        branch_mode,
        // Subprocess: profile patterns arrive via DEVDOCK_FORCE_SYNC_PATTERNS, so no
        // extra layer is needed here.
        Vec::new(),
        sync_matcher,
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

fn matcher_policy_snapshot(sync_matcher: &SyncMatcher) -> Result<Option<String>> {
    let Some(policy) = sync_matcher.repository_policy() else {
        return Ok(None);
    };

    let snapshot = serde_json::to_vec(policy).map_err(|error| {
        SyncError::DaemonError(format!("Failed to serialize sync policy snapshot: {error}"))
    })?;
    RepositorySyncPolicyV1::from_snapshot(&snapshot).map_err(|error| {
        SyncError::DaemonError(format!("Refusing invalid sync policy snapshot: {error}"))
    })?;
    String::from_utf8(snapshot).map(Some).map_err(|error| {
        SyncError::DaemonError(format!("Sync policy snapshot was not UTF-8: {error}"))
    })
}

fn matcher_from_snapshot(snapshot: Option<&str>) -> Result<SyncMatcher> {
    let Some(snapshot) = snapshot else {
        return Ok(SyncMatcher::universal());
    };

    let policy = RepositorySyncPolicyV1::from_snapshot(snapshot.as_bytes()).map_err(|error| {
        SyncError::DaemonError(format!("Invalid sync policy snapshot from parent: {error}"))
    })?;
    Ok(SyncMatcher::with_repository_policy(&policy))
}

fn matcher_from_process_env() -> Result<SyncMatcher> {
    match std::env::var(SYNC_POLICY_SNAPSHOT_ENV_VAR) {
        Ok(snapshot) => matcher_from_snapshot(Some(&snapshot)),
        Err(std::env::VarError::NotPresent) => matcher_from_snapshot(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(SyncError::DaemonError(format!(
            "{SYNC_POLICY_SNAPSHOT_ENV_VAR} is not valid UTF-8"
        ))),
    }
}

/// Stop sync daemon and clean up all related files.
pub fn stop_daemon(devdock_id: &str) -> Result<()> {
    if let Some(pid) = read_daemon_pid(devdock_id)? {
        if is_daemon_running(devdock_id, pid) {
            // A PID file alone is never kill authority. The command/ID check
            // above proves this is our daemon before it receives a signal.
            #[cfg(unix)]
            {
                use std::process::Command;
                Command::new("kill").arg(pid.to_string()).status().map_err(|error| {
                    SyncError::DaemonError(format!(
                        "failed to signal owned sync daemon {pid}: {error}"
                    ))
                })?;
            }

            #[cfg(windows)]
            {
                use std::process::Command;
                Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).status().map_err(
                    |error| {
                        SyncError::DaemonError(format!(
                            "failed to signal owned sync daemon {pid}: {error}"
                        ))
                    },
                )?;
            }

            let deadline = Instant::now() + Duration::from_secs(5);
            while is_process_alive(pid) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            if is_process_alive(pid) {
                return Err(SyncError::DaemonError(format!(
                    "owned sync daemon {pid} did not exit within 5 seconds"
                )));
            }
        }
    }
    cleanup_sync_files(devdock_id)
}

/// Clean up all sync files for a specific devdock instance.
pub fn cleanup_sync_files(devdock_id: &str) -> Result<()> {
    let pid_dir = SyncDaemon::get_pid_dir();
    let pid_file = pid_dir.join(format!("{devdock_id}.pid"));

    if pid_file.exists() {
        let pid_str = fs::read_to_string(&pid_file)?;
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            if is_process_alive(pid) && !is_daemon_running(devdock_id, pid) {
                return Err(SyncError::DaemonError(format!(
                    "refusing to remove sync evidence for live unproven PID {pid}"
                )));
            }
            if is_daemon_running(devdock_id, pid) {
                return Err(SyncError::DaemonError(format!(
                    "refusing to remove sync evidence while owned daemon {pid} is still running"
                )));
            }
        }
    }

    // Clean up all sync files for this devdock_id
    let sync_files = vec![
        format!("{}.pid", devdock_id),
        format!("{}.excludes", devdock_id),
        format!("{}.hb", devdock_id),
        format!("{}.stats", devdock_id),
        format!("{}.stats.json", devdock_id),
        format!("{}.history.jsonl", devdock_id),
        format!("{}.errors.jsonl", devdock_id),
        format!("{}.initial-sync.json", devdock_id),
        format!("{}.queue.json", devdock_id),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn repository_policy() -> RepositorySyncPolicyV1 {
        RepositorySyncPolicyV1 {
            schema_version: 1,
            compatible_since_sha: "a".repeat(40),
            monitored_roots: vec!["terraform".into()],
            sync_extensions: vec![".tf".into()],
            sync_filenames: vec!["BUILD".into()],
            impact_roots: vec![],
            benign_out_of_scope: vec![],
        }
    }

    #[test]
    fn parse_pid_accepts_trimmed_number() {
        assert_eq!(parse_pid("12345\n"), Some(12345));
    }

    #[test]
    fn parse_pid_rejects_invalid_content() {
        assert_eq!(parse_pid("abc"), None);
        assert_eq!(parse_pid(""), None);
    }

    #[test]
    fn command_matches_expected_daemon() {
        let command = "/path/devdock __sync_daemon__ dev-123 workspace /repo false false info";
        assert!(command_matches_daemon(command, "dev-123"));
    }

    #[test]
    fn command_rejects_unrelated_process() {
        assert!(!command_matches_daemon("some unrelated process", "dev-123"));
    }

    #[test]
    fn command_rejects_different_devdock_id() {
        let command = "/path/devdock __sync_daemon__ dev-999 workspace /repo false false info";
        assert!(!command_matches_daemon(command, "dev-123"));
    }

    #[test]
    fn repository_matcher_round_trips_through_daemon_snapshot() {
        let matcher = SyncMatcher::with_repository_policy(&repository_policy());
        let snapshot = matcher_policy_snapshot(&matcher).unwrap().unwrap();
        let child_matcher = matcher_from_snapshot(Some(&snapshot)).unwrap();

        assert!(child_matcher.is_syncable(Path::new("terraform/main.tf")));
        assert!(child_matcher.is_syncable(Path::new("go/service/BUILD")));
    }

    #[test]
    fn missing_daemon_snapshot_preserves_universal_compatibility() {
        let child_matcher = matcher_from_snapshot(None).unwrap();

        assert!(child_matcher.is_syncable(Path::new("python_scio/service/main.py")));
        assert!(!child_matcher.is_syncable(Path::new("terraform/main.tf")));
    }

    #[test]
    fn invalid_daemon_snapshot_fails_closed() {
        let error = matcher_from_snapshot(Some(r#"{"schema_version":999}"#)).unwrap_err();

        assert!(error.to_string().contains("Invalid sync policy snapshot from parent"));
    }

    #[cfg(unix)]
    #[test]
    fn detached_daemon_survives_a_launch_wrapper_exit_in_its_own_session() {
        let caller_session = unsafe { libc::getsid(0) };
        assert_ne!(caller_session, -1);

        let temp_dir = tempfile::tempdir().unwrap();
        let child_pid_path = temp_dir.path().join("daemon-child.pid");
        let mut command = std::process::Command::new("sh");
        // The shell models a noninteractive launcher. It exits immediately
        // after starting a daemon-like child, which must remain alive in the
        // independent session established by `spawn_detached_daemon`.
        command
            .arg("-c")
            .arg("sleep 5 & printf '%s' \"$!\" > \"$1\"")
            .arg("daemon-launcher")
            .arg(&child_pid_path);
        let mut launcher = spawn_detached_daemon(&mut command).unwrap();
        let launcher_pid = launcher.id() as libc::pid_t;
        assert!(launcher.wait().unwrap().success());

        let daemon_pid = std::fs::read_to_string(&child_pid_path)
            .unwrap()
            .trim()
            .parse::<libc::pid_t>()
            .unwrap();
        let daemon_session = unsafe { libc::getsid(daemon_pid) };
        assert_eq!(daemon_session, launcher_pid);
        assert_ne!(daemon_session, caller_session);
        assert_eq!(
            unsafe { libc::kill(daemon_pid, 0) },
            0,
            "daemon remains alive after its launch wrapper exits"
        );

        let _ = unsafe { libc::kill(daemon_pid, libc::SIGTERM) };
    }
}
