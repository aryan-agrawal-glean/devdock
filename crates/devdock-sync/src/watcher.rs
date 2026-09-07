//! File system watcher with debouncing.

use crate::Result;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// File watcher with debouncing for file change events.
pub struct FileWatcher {
    watcher: RecommendedWatcher,
    rx: Receiver<notify::Result<Event>>,
    debounce_duration: Duration,
}

impl FileWatcher {
    /// Create a new file watcher.
    pub fn new() -> Result<Self> {
        let (tx, rx) = channel();

        let watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            Config::default(),
        )?;

        Ok(Self { watcher, rx, debounce_duration: Duration::from_millis(500) })
    }

    /// Start watching a path.
    pub fn watch(&mut self, path: &Path) -> Result<()> {
        info!("Starting file watcher on {:?}", path);
        self.watcher.watch(path, RecursiveMode::Recursive)?;
        Ok(())
    }

    /// Get changed files with debouncing.
    /// Returns a batch after the filesystem has been quiet for the debounce window.
    pub fn get_changed_files(&self, timeout: Duration) -> Result<Vec<PathBuf>> {
        let mut changed_files: HashMap<PathBuf, Instant> = HashMap::new();
        let start = Instant::now();
        let mut last_event_at: Option<Instant> = None;

        // Wait for the first event, then extend the receive window after every event.
        // The old fixed timeout returned immediately after the first quiet period and
        // discarded events newer than debounce_duration, which made ordinary edits
        // disappear from sync history.
        loop {
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                break;
            }
            let wait = last_event_at
                .map(|event_at| self.debounce_duration.saturating_sub(event_at.elapsed()))
                .unwrap_or(remaining)
                .min(remaining);
            match self.rx.recv_timeout(wait) {
                Ok(Ok(event)) => {
                    let event_at = Instant::now();
                    for path in event.paths {
                        changed_files.insert(path, event_at);
                    }
                    last_event_at = Some(event_at);
                }
                Ok(Err(e)) => {
                    debug!("Watcher error: {}", e);
                }
                Err(_) => {
                    // No event arrived during the quiet window.
                    break;
                }
            }
        }

        let files: Vec<PathBuf> = changed_files.into_keys().collect();

        if !files.is_empty() {
            debug!("Detected {} changed files", files.len());
        }

        Ok(files)
    }

    /// Stop watching.
    pub fn stop(&mut self) -> Result<()> {
        // RecommendedWatcher will be dropped, automatically stopping watching
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::FileWatcher;
    use std::fs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn reports_an_edit_after_the_quiet_window() {
        let suffix =
            SystemTime::now().duration_since(UNIX_EPOCH).expect("clock before epoch").as_nanos();
        let dir = std::env::temp_dir().join(format!("devdock-sync-watcher-{suffix}"));
        fs::create_dir_all(&dir).expect("create temp directory");
        let path = dir.join("sentinel.yaml");
        fs::write(&path, "before\n").expect("write initial file");

        let mut watcher = FileWatcher::new().expect("create watcher");
        watcher.watch(&dir).expect("watch temp directory");
        fs::write(&path, "after\n").expect("write sentinel");

        let changed =
            watcher.get_changed_files(Duration::from_secs(3)).expect("collect changed files");
        assert!(
            changed.iter().any(|candidate| candidate.ends_with("sentinel.yaml")),
            "changed files: {changed:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
