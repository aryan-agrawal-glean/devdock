//! Collect remote-start logs and failure stage for bug reports.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Canonical latest-start log on a Coder workspace (symlink to the per-session file).
pub const CANONICAL_START_LOG: &str = "/tmp/devdock-start.log";
pub const CANONICAL_START_META: &str = "/tmp/devdock-start.meta";
pub const CANONICAL_START_EXIT: &str = "/tmp/devdock-start.exit";

const MAX_LOG_BYTES: usize = 256 * 1024;

/// Remote start failure context attached to `devdock report`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupDiagnostics {
    pub failure_stage: Option<String>,
    pub failure_reason: Option<String>,
    pub exit_status: Option<String>,
    pub log_path: Option<String>,
    pub log_content: Option<String>,
}

impl StartupDiagnostics {
    pub fn is_empty(&self) -> bool {
        self.failure_stage.is_none()
            && self.failure_reason.is_none()
            && self.exit_status.is_none()
            && self.log_content.is_none()
    }

    /// Files to include in the report archive when present.
    pub fn archive_files(&self) -> HashMap<String, String> {
        let mut files = HashMap::new();
        if let Some(content) = &self.log_content {
            files.insert("remote_start.log".to_string(), content.clone());
        }
        let mut meta = String::new();
        if let Some(stage) = &self.failure_stage {
            meta.push_str(&format!("failure_stage={stage}\n"));
        }
        if let Some(reason) = &self.failure_reason {
            meta.push_str(&format!("failure_reason={reason}\n"));
        }
        if let Some(exit) = &self.exit_status {
            meta.push_str(&format!("exit_status={exit}\n"));
        }
        if let Some(path) = &self.log_path {
            meta.push_str(&format!("log_path={path}\n"));
        }
        if !meta.is_empty() {
            files.insert("failure_stage.txt".to_string(), meta);
        }
        files
    }
}

/// Collect start-log + failure-stage from env, explicit CLI context, and default remote paths.
pub fn collect_startup_diagnostics(
    failure_stage: Option<String>,
    failure_reason: Option<String>,
) -> StartupDiagnostics {
    let stage = first_nonempty(failure_stage, std::env::var("DEVDOCK_FAILURE_STAGE").ok());
    let reason = first_nonempty(failure_reason, std::env::var("DEVDOCK_FAILURE_REASON").ok());
    let log_paths = discover_start_logs();
    let meta_paths = discover_start_meta();
    collect_startup_diagnostics_from(
        &log_paths,
        &meta_paths,
        Some(Path::new(CANONICAL_START_EXIT)),
        stage,
        reason,
    )
}

pub fn collect_startup_diagnostics_from(
    log_paths: &[PathBuf],
    meta_paths: &[PathBuf],
    exit_path: Option<&Path>,
    failure_stage: Option<String>,
    failure_reason: Option<String>,
) -> StartupDiagnostics {
    let mut diag =
        StartupDiagnostics { failure_stage, failure_reason, ..StartupDiagnostics::default() };

    if let Some(path) = log_paths.iter().find(|p| p.exists()) {
        if let Some(content) = read_bounded(path, MAX_LOG_BYTES) {
            diag.log_path = Some(path.display().to_string());
            diag.log_content = Some(content);
        } else if path.exists() {
            // File exists but is empty — still record the path so reports can cite it.
            diag.log_path = Some(path.display().to_string());
        }
    }

    for meta_path in meta_paths {
        if let Ok(text) = fs::read_to_string(meta_path) {
            apply_meta(&mut diag, &text);
            break;
        }
    }

    if diag.exit_status.is_none() {
        if let Some(path) = exit_path {
            if let Ok(text) = fs::read_to_string(path) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    diag.exit_status = Some(trimmed.to_string());
                }
            }
        }
    }

    diag
}

fn first_nonempty(primary: Option<String>, fallback: Option<String>) -> Option<String> {
    primary.filter(|s| !s.trim().is_empty()).or_else(|| fallback.filter(|s| !s.trim().is_empty()))
}

fn discover_start_logs() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from(CANONICAL_START_LOG)];
    if let Ok(entries) = fs::read_dir("/tmp") {
        let mut extra: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("devdock-start-") && n.ends_with(".log"))
                    .unwrap_or(false)
            })
            .collect();
        extra.sort();
        paths.extend(extra);
    }
    paths
}

fn discover_start_meta() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from(CANONICAL_START_META)];
    if let Ok(entries) = fs::read_dir("/tmp") {
        let mut extra: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("devdock-start-") && n.ends_with(".meta"))
                    .unwrap_or(false)
            })
            .collect();
        extra.sort();
        paths.extend(extra);
    }
    paths
}

fn apply_meta(diag: &mut StartupDiagnostics, text: &str) {
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("stage=") {
            if diag.failure_stage.is_none() && !value.trim().is_empty() {
                diag.failure_stage = Some(value.trim().to_string());
            }
        } else if let Some(value) = line.strip_prefix("failure_stage=") {
            if diag.failure_stage.is_none() && !value.trim().is_empty() {
                diag.failure_stage = Some(value.trim().to_string());
            }
        } else if let Some(value) = line.strip_prefix("exit_code=") {
            if diag.exit_status.is_none() && !value.trim().is_empty() {
                diag.exit_status = Some(value.trim().to_string());
            }
        } else if let Some(value) = line.strip_prefix("log=") {
            if diag.log_path.is_none() && !value.trim().is_empty() {
                diag.log_path = Some(value.trim().to_string());
            }
        }
    }
}

fn read_bounded(path: &Path, max_bytes: usize) -> Option<String> {
    let data = fs::read(path).ok()?;
    if data.is_empty() {
        return None;
    }
    let slice = if data.len() > max_bytes {
        let start = data.len() - max_bytes;
        let start =
            (start..data.len()).find(|&i| data[i] & 0b1100_0000 != 0b1000_0000).unwrap_or(start);
        &data[start..]
    } else {
        data.as_slice()
    };
    Some(String::from_utf8_lossy(slice).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(suffix: &str) -> PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("devdock-diag-test-{nanos}-{suffix}"))
    }

    #[test]
    fn collects_log_and_failure_stage_from_files() {
        let log_path = temp_path("start.log");
        let meta_path = temp_path("start.meta");
        let exit_path = temp_path("start.exit");

        let mut log = File::create(&log_path).unwrap();
        writeln!(log, "child exploded").unwrap();
        let mut meta = File::create(&meta_path).unwrap();
        writeln!(meta, "stage=remote_devdock_start").unwrap();
        writeln!(meta, "exit_code=9").unwrap();
        let mut exit = File::create(&exit_path).unwrap();
        writeln!(exit, "9").unwrap();

        let diag = collect_startup_diagnostics_from(
            std::slice::from_ref(&log_path),
            std::slice::from_ref(&meta_path),
            Some(&exit_path),
            None,
            Some("tmux_dead".into()),
        );

        assert_eq!(diag.failure_stage.as_deref(), Some("remote_devdock_start"));
        assert_eq!(diag.failure_reason.as_deref(), Some("tmux_dead"));
        assert_eq!(diag.exit_status.as_deref(), Some("9"));
        assert!(diag.log_content.as_deref().unwrap().contains("child exploded"));
        let files = diag.archive_files();
        assert!(files.get("remote_start.log").unwrap().contains("child exploded"));
        assert!(files
            .get("failure_stage.txt")
            .unwrap()
            .contains("failure_stage=remote_devdock_start"));

        let _ = fs::remove_file(log_path);
        let _ = fs::remove_file(meta_path);
        let _ = fs::remove_file(exit_path);
    }

    #[test]
    fn explicit_failure_stage_wins_over_meta() {
        let meta_path = temp_path("start.meta");
        let mut meta = File::create(&meta_path).unwrap();
        writeln!(meta, "stage=from-meta").unwrap();

        let diag = collect_startup_diagnostics_from(
            &[],
            std::slice::from_ref(&meta_path),
            None,
            Some("from-cli".into()),
            None,
        );
        assert_eq!(diag.failure_stage.as_deref(), Some("from-cli"));
        let _ = fs::remove_file(meta_path);
    }

    #[test]
    fn missing_files_yield_empty_diagnostics() {
        let missing = PathBuf::from("/tmp/devdock-does-not-exist-start.log");
        let diag = collect_startup_diagnostics_from(&[missing], &[], None, None, None);
        assert!(diag.is_empty());
        assert!(diag.archive_files().is_empty());
    }
}
