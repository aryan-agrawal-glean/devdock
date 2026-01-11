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
    /// Returns a batch of files that changed within the debounce window.
    pub fn get_changed_files(&self, timeout: Duration) -> Result<Vec<PathBuf>> {
        let mut changed_files: HashMap<PathBuf, Instant> = HashMap::new();
        let start = Instant::now();

        // Collect events for debounce duration
        while start.elapsed() < timeout {
            match self.rx.recv_timeout(self.debounce_duration) {
                Ok(Ok(event)) => {
                    for path in event.paths {
                        changed_files.insert(path.clone(), Instant::now());
                    }
                }
                Ok(Err(e)) => {
                    debug!("Watcher error: {}", e);
                }
                Err(_) => {
                    // Timeout - check if we have any changes
                    break;
                }
            }
        }

        // Filter out very recent changes (still being written)
        let now = Instant::now();
        let files: Vec<PathBuf> = changed_files
            .into_iter()
            .filter(|(_, time)| now.duration_since(*time) >= self.debounce_duration)
            .map(|(path, _)| path)
            .collect();

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
