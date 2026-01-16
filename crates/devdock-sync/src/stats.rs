//! Sync statistics tracking with async background writes.

use crate::{Result, SyncError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Flush interval: write stats to disk every 20 seconds
const FLUSH_INTERVAL_SECS: u64 = 20;

/// Flush batch size: write stats after 10 syncs
const FLUSH_BATCH_SIZE: usize = 10;

/// Maximum history entries to keep
const MAX_HISTORY_ENTRIES: usize = 10_000;

/// Maximum error entries to keep
const MAX_ERROR_ENTRIES: usize = 100;

/// Stats update message sent to background writer
#[derive(Debug, Clone)]
pub struct StatsUpdate {
    pub files: Vec<PathBuf>,
    pub bytes: u64,
    pub duration_ms: u64,
    pub success: bool,
    pub error: Option<String>,
}

/// Session statistics (written to .stats.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    pub session_start: String,
    pub last_sync: Option<String>,
    pub total_files_synced: u64,
    pub total_bytes_synced: u64,
    pub successful_syncs: u32,
    pub failed_syncs: u32,
    pub current_status: String,
}

/// Sync history entry (written to .history.jsonl)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub ts: String,
    pub files: Vec<String>,
    pub count: usize,
    pub bytes: u64,
    pub dur_ms: u64,
    pub ok: bool,
}

/// Error entry (written to .errors.jsonl)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEntry {
    pub ts: String,
    pub files: Vec<String>,
    pub error: String,
}

/// Sync statistics tracker with async background writes
pub struct SyncStatsTracker {
    // In-memory counters (lock-free atomics)
    session_start: Instant,
    session_start_unix: i64,
    total_files_synced: Arc<AtomicU64>,
    total_bytes_synced: Arc<AtomicU64>,
    successful_syncs: Arc<AtomicU32>,
    failed_syncs: Arc<AtomicU32>,
    last_sync_time: Arc<Mutex<Option<Instant>>>,

    // Current queue state
    pending_files: Arc<Mutex<Vec<PathBuf>>>,
    syncing_files: Arc<Mutex<Vec<PathBuf>>>,

    // Channel to background writer
    update_tx: mpsc::UnboundedSender<StatsUpdate>,

    // File paths
    stats_file: PathBuf,
    history_file: PathBuf,
    errors_file: PathBuf,
    queue_file: PathBuf,
}

impl SyncStatsTracker {
    /// Create a new stats tracker and spawn background writer thread
    pub fn new(devdock_id: &str) -> Result<Self> {
        let sync_dir = Self::get_sync_dir();
        std::fs::create_dir_all(&sync_dir)?;

        let stats_file = sync_dir.join(format!("{}.stats.json", devdock_id));
        let history_file = sync_dir.join(format!("{}.history.jsonl", devdock_id));
        let errors_file = sync_dir.join(format!("{}.errors.jsonl", devdock_id));
        let queue_file = sync_dir.join(format!("{}.queue.json", devdock_id));

        let (update_tx, update_rx) = mpsc::unbounded_channel();

        let now = Instant::now();
        let now_unix =
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);

        let tracker = Self {
            session_start: now,
            session_start_unix: now_unix,
            total_files_synced: Arc::new(AtomicU64::new(0)),
            total_bytes_synced: Arc::new(AtomicU64::new(0)),
            successful_syncs: Arc::new(AtomicU32::new(0)),
            failed_syncs: Arc::new(AtomicU32::new(0)),
            last_sync_time: Arc::new(Mutex::new(None)),
            pending_files: Arc::new(Mutex::new(Vec::new())),
            syncing_files: Arc::new(Mutex::new(Vec::new())),
            update_tx,
            stats_file: stats_file.clone(),
            history_file: history_file.clone(),
            errors_file: errors_file.clone(),
            queue_file: queue_file.clone(),
        };

        // Spawn background writer thread
        let stats_file_clone = stats_file.clone();
        let history_file_clone = history_file.clone();
        let errors_file_clone = errors_file.clone();
        let session_start_unix_clone = now_unix;
        let total_files = tracker.total_files_synced.clone();
        let total_bytes = tracker.total_bytes_synced.clone();
        let successful = tracker.successful_syncs.clone();
        let failed = tracker.failed_syncs.clone();
        let last_sync = tracker.last_sync_time.clone();

        tokio::spawn(async move {
            let ctx = StatsWriterContext {
                stats_file: stats_file_clone,
                history_file: history_file_clone,
                errors_file: errors_file_clone,
                session_start_unix: session_start_unix_clone,
                total_files,
                total_bytes,
                successful,
                failed,
                last_sync,
            };
            stats_writer_loop(update_rx, ctx).await;
        });

        info!("Stats tracker initialized for {}", devdock_id);
        Ok(tracker)
    }

    /// Record a successful sync (instant, lock-free)
    pub fn record_success(&self, file_count: usize, bytes: u64) {
        self.total_files_synced.fetch_add(file_count as u64, Ordering::Relaxed);
        self.total_bytes_synced.fetch_add(bytes, Ordering::Relaxed);
        self.successful_syncs.fetch_add(1, Ordering::Relaxed);

        if let Ok(mut last_sync) = self.last_sync_time.lock() {
            *last_sync = Some(Instant::now());
        }
    }

    /// Record a failed sync (instant, lock-free)
    pub fn record_failure(&self) {
        self.failed_syncs.fetch_add(1, Ordering::Relaxed);

        if let Ok(mut last_sync) = self.last_sync_time.lock() {
            *last_sync = Some(Instant::now());
        }
    }

    /// Send update to background writer (non-blocking)
    pub fn send_update(&self, update: StatsUpdate) {
        // This is a non-blocking send, takes ~50 microseconds
        if let Err(e) = self.update_tx.send(update) {
            error!("Failed to send stats update: {}", e);
        }
    }

    /// Get current session statistics (instant read from atomics)
    pub fn get_session_stats(&self) -> SessionStats {
        let last_sync = self.last_sync_time.lock().ok().and_then(|guard| *guard).map(|instant| {
            let elapsed = instant.elapsed();
            let unix_time = self.session_start_unix + self.session_start.elapsed().as_secs() as i64
                - elapsed.as_secs() as i64;
            format_timestamp(unix_time)
        });

        SessionStats {
            session_start: format_timestamp(self.session_start_unix),
            last_sync,
            total_files_synced: self.total_files_synced.load(Ordering::Relaxed),
            total_bytes_synced: self.total_bytes_synced.load(Ordering::Relaxed),
            successful_syncs: self.successful_syncs.load(Ordering::Relaxed),
            failed_syncs: self.failed_syncs.load(Ordering::Relaxed),
            current_status: "running".to_string(),
        }
    }

    /// Get uptime duration
    pub fn uptime(&self) -> Duration {
        self.session_start.elapsed()
    }

    /// Get time since last sync
    pub fn time_since_last_sync(&self) -> Option<Duration> {
        self.last_sync_time.lock().ok().and_then(|guard| *guard).map(|instant| instant.elapsed())
    }

    /// Get sync directory path
    fn get_sync_dir() -> PathBuf {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join(".devdock/sync")
    }

    /// Get stats file path
    pub fn stats_file_path(&self) -> &Path {
        &self.stats_file
    }

    /// Get history file path
    pub fn history_file_path(&self) -> &Path {
        &self.history_file
    }

    /// Get errors file path
    pub fn errors_file_path(&self) -> &Path {
        &self.errors_file
    }

    /// Set pending files (files detected by watcher, waiting to sync)
    pub fn set_pending_files(&self, files: Vec<PathBuf>) {
        if let Ok(mut pending) = self.pending_files.lock() {
            *pending = files;
        }
        // Write queue file immediately for status visibility
        let _ = self.write_queue_file();
    }

    /// Mark files as currently syncing
    pub fn start_syncing(&self, files: &[PathBuf]) {
        if let Ok(mut syncing) = self.syncing_files.lock() {
            *syncing = files.to_vec();
        }
        if let Ok(mut pending) = self.pending_files.lock() {
            // Remove from pending
            pending.retain(|f| !files.contains(f));
        }
        let _ = self.write_queue_file();
    }

    /// Clear syncing files after completion
    pub fn finish_syncing(&self) {
        if let Ok(mut syncing) = self.syncing_files.lock() {
            syncing.clear();
        }
        let _ = self.write_queue_file();
    }

    /// Get current queue state
    pub fn get_queue_state(&self) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let pending = self.pending_files.lock().ok().map(|p| p.clone()).unwrap_or_default();
        let syncing = self.syncing_files.lock().ok().map(|s| s.clone()).unwrap_or_default();
        (pending, syncing)
    }

    /// Write queue file (for status visibility)
    fn write_queue_file(&self) -> Result<()> {
        use serde_json::json;

        let (pending, syncing) = self.get_queue_state();

        // If queue is empty, delete the file
        if pending.is_empty() && syncing.is_empty() {
            let _ = std::fs::remove_file(&self.queue_file);
            return Ok(());
        }

        let queue_data = json!({
            "pending": pending.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "in_progress": syncing.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "last_update": format_timestamp(current_timestamp()),
        });

        let json_str = serde_json::to_string_pretty(&queue_data)
            .map_err(|e| SyncError::DaemonError(format!("Failed to serialize queue: {}", e)))?;

        std::fs::write(&self.queue_file, json_str)?;
        Ok(())
    }
}

/// Context for stats writer loop
struct StatsWriterContext {
    stats_file: PathBuf,
    history_file: PathBuf,
    errors_file: PathBuf,
    session_start_unix: i64,
    total_files: Arc<AtomicU64>,
    total_bytes: Arc<AtomicU64>,
    successful: Arc<AtomicU32>,
    failed: Arc<AtomicU32>,
    last_sync: Arc<Mutex<Option<Instant>>>,
}

/// Background writer loop - handles all disk I/O
async fn stats_writer_loop(mut rx: mpsc::UnboundedReceiver<StatsUpdate>, ctx: StatsWriterContext) {
    let mut buffer = Vec::new();
    let mut last_flush = Instant::now();

    info!("Stats writer loop started");

    loop {
        // Wait up to 20 seconds or until we get an update
        let timeout_result =
            tokio::time::timeout(Duration::from_secs(FLUSH_INTERVAL_SECS), rx.recv()).await;

        match timeout_result {
            Ok(Some(update)) => {
                buffer.push(update);

                // Flush if we've accumulated 10 syncs OR it's been 20s
                if buffer.len() >= FLUSH_BATCH_SIZE
                    || last_flush.elapsed() >= Duration::from_secs(FLUSH_INTERVAL_SECS)
                {
                    debug!("Flushing {} stats updates to disk", buffer.len());
                    flush_to_disk(&buffer, &ctx).await;
                    buffer.clear();
                    last_flush = Instant::now();
                }
            }
            Ok(None) => {
                // Channel closed - flush remaining and exit
                info!("Stats writer channel closed, flushing remaining updates");
                if !buffer.is_empty() {
                    flush_to_disk(&buffer, &ctx).await;
                }
                break;
            }
            Err(_) => {
                // Timeout (20s passed) - flush whatever we have
                if !buffer.is_empty() {
                    debug!("Flush timer expired, flushing {} updates", buffer.len());
                    flush_to_disk(&buffer, &ctx).await;
                    buffer.clear();
                }
                last_flush = Instant::now();
            }
        }
    }

    info!("Stats writer loop exited");
}

/// Flush buffered updates to disk
async fn flush_to_disk(updates: &[StatsUpdate], ctx: &StatsWriterContext) {
    // 1. Write session stats (atomic via temp file)
    if let Err(e) = write_session_stats(
        &ctx.stats_file,
        ctx.session_start_unix,
        &ctx.total_files,
        &ctx.total_bytes,
        &ctx.successful,
        &ctx.failed,
        &ctx.last_sync,
    )
    .await
    {
        error!("Failed to write session stats: {}", e);
    }

    // 2. Append to history file
    if let Err(e) = append_history_entries(&ctx.history_file, updates).await {
        error!("Failed to append history entries: {}", e);
    }

    // 3. Append errors (if any)
    let errors: Vec<_> = updates.iter().filter(|u| !u.success).collect();
    if !errors.is_empty() {
        if let Err(e) = append_error_entries(&ctx.errors_file, &errors).await {
            error!("Failed to append error entries: {}", e);
        }
    }

    // 4. Rotate files if needed
    if let Err(e) = rotate_file_if_needed(&ctx.history_file, MAX_HISTORY_ENTRIES).await {
        warn!("Failed to rotate history file: {}", e);
    }
    if let Err(e) = rotate_file_if_needed(&ctx.errors_file, MAX_ERROR_ENTRIES).await {
        warn!("Failed to rotate errors file: {}", e);
    }
}

/// Write session stats to file (atomic via temp file)
async fn write_session_stats(
    stats_file: &Path,
    session_start_unix: i64,
    total_files: &Arc<AtomicU64>,
    total_bytes: &Arc<AtomicU64>,
    successful: &Arc<AtomicU32>,
    failed: &Arc<AtomicU32>,
    last_sync: &Arc<Mutex<Option<Instant>>>,
) -> Result<()> {
    let last_sync_time = last_sync
        .lock()
        .ok()
        .and_then(|guard| *guard)
        .map(|_| format_timestamp(current_timestamp()));

    let stats = SessionStats {
        session_start: format_timestamp(session_start_unix),
        last_sync: last_sync_time,
        total_files_synced: total_files.load(Ordering::Relaxed),
        total_bytes_synced: total_bytes.load(Ordering::Relaxed),
        successful_syncs: successful.load(Ordering::Relaxed),
        failed_syncs: failed.load(Ordering::Relaxed),
        current_status: "running".to_string(),
    };

    let json = serde_json::to_string_pretty(&stats)
        .map_err(|e| SyncError::DaemonError(format!("Failed to serialize stats: {}", e)))?;

    // Atomic write: write to temp file, then rename
    let temp_file = stats_file.with_extension("tmp");
    tokio::fs::write(&temp_file, json).await?;
    tokio::fs::rename(&temp_file, stats_file).await?;

    Ok(())
}

/// Append history entries to file
async fn append_history_entries(history_file: &Path, updates: &[StatsUpdate]) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let mut file =
        tokio::fs::OpenOptions::new().create(true).append(true).open(history_file).await?;

    for update in updates {
        let entry = HistoryEntry {
            ts: format_timestamp(current_timestamp()),
            files: update.files.iter().map(|p| p.display().to_string()).collect(),
            count: update.files.len(),
            bytes: update.bytes,
            dur_ms: update.duration_ms,
            ok: update.success,
        };

        let json = serde_json::to_string(&entry)
            .map_err(|e| SyncError::DaemonError(format!("Failed to serialize history: {}", e)))?;

        file.write_all(json.as_bytes()).await?;
        file.write_all(b"\n").await?;
    }

    file.flush().await?;
    Ok(())
}

/// Append error entries to file
async fn append_error_entries(errors_file: &Path, errors: &[&StatsUpdate]) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let mut file =
        tokio::fs::OpenOptions::new().create(true).append(true).open(errors_file).await?;

    for update in errors {
        let entry = ErrorEntry {
            ts: format_timestamp(current_timestamp()),
            files: update.files.iter().map(|p| p.display().to_string()).collect(),
            error: update.error.clone().unwrap_or_else(|| "Unknown error".to_string()),
        };

        let json = serde_json::to_string(&entry)
            .map_err(|e| SyncError::DaemonError(format!("Failed to serialize error: {}", e)))?;

        file.write_all(json.as_bytes()).await?;
        file.write_all(b"\n").await?;
    }

    file.flush().await?;
    Ok(())
}

/// Rotate file if it exceeds max entries
async fn rotate_file_if_needed(file_path: &Path, max_entries: usize) -> Result<()> {
    if !file_path.exists() {
        return Ok(());
    }

    let content = tokio::fs::read_to_string(file_path).await?;
    let line_count = content.lines().count();

    if line_count > max_entries {
        // Keep only the last max_entries lines
        let lines: Vec<_> = content.lines().collect();
        let keep_from = line_count.saturating_sub(max_entries);
        let new_content = lines[keep_from..].join("\n") + "\n";

        tokio::fs::write(file_path, new_content).await?;
        info!("Rotated {} from {} to {} entries", file_path.display(), line_count, max_entries);
    }

    Ok(())
}

/// Format Unix timestamp as ISO 8601 string
fn format_timestamp(unix_secs: i64) -> String {
    use chrono::{DateTime, Utc};
    let dt = DateTime::<Utc>::from_timestamp(unix_secs, 0).unwrap_or_else(Utc::now);
    dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Get current Unix timestamp
fn current_timestamp() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Read session stats from file
pub fn read_session_stats(devdock_id: &str) -> Result<SessionStats> {
    let sync_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join(".devdock/sync");
    let stats_file = sync_dir.join(format!("{}.stats.json", devdock_id));

    if !stats_file.exists() {
        return Err(SyncError::DaemonError("Stats file not found".to_string()));
    }

    let content = std::fs::read_to_string(&stats_file)?;
    let stats: SessionStats = serde_json::from_str(&content)
        .map_err(|e| SyncError::DaemonError(format!("Failed to parse stats: {}", e)))?;

    Ok(stats)
}

/// Read history entries from file
pub fn read_history_entries(devdock_id: &str, tail: Option<usize>) -> Result<Vec<HistoryEntry>> {
    let sync_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join(".devdock/sync");
    let history_file = sync_dir.join(format!("{}.history.jsonl", devdock_id));

    if !history_file.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&history_file)?;
    let mut entries: Vec<HistoryEntry> =
        content.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();

    // Return last N entries if tail is specified
    if let Some(n) = tail {
        let start = entries.len().saturating_sub(n);
        entries = entries[start..].to_vec();
    }

    Ok(entries)
}

/// Read error entries from file
pub fn read_error_entries(devdock_id: &str, tail: Option<usize>) -> Result<Vec<ErrorEntry>> {
    let sync_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join(".devdock/sync");
    let errors_file = sync_dir.join(format!("{}.errors.jsonl", devdock_id));

    if !errors_file.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&errors_file)?;
    let mut entries: Vec<ErrorEntry> =
        content.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();

    // Return last N entries if tail is specified
    if let Some(n) = tail {
        let start = entries.len().saturating_sub(n);
        entries = entries[start..].to_vec();
    }

    Ok(entries)
}
