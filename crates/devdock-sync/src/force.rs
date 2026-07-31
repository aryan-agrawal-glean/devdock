//! Force-sync: resolve patterns and sync files bypassing normal filters.

use crate::rsync::RsyncClient;
use crate::{Result, SyncError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Persisted force-watch patterns.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ForcePatterns {
    /// Glob patterns that the daemon should watch and sync without filtering.
    pub patterns: Vec<String>,
}

/// Result of resolving force-sync paths/patterns to concrete files.
#[derive(Debug)]
pub struct ResolvedFiles {
    /// Concrete relative file paths ready for rsync.
    pub files: Vec<PathBuf>,
    /// Patterns that did not match any files (for user warning).
    pub unmatched_patterns: Vec<String>,
}

/// Resolve a list of user-provided paths/globs to concrete file paths.
///
/// For each entry in `raw_paths`:
///   - If it contains glob metacharacters (`*`, `?`, `[`), expand it relative to `repo_path`.
///   - If it is a directory, recursively collect all files in it.
///   - If it is a plain file, include it directly.
///
/// All returned paths are relative to `repo_path`.
pub fn resolve_paths(repo_path: &Path, raw_paths: &[String]) -> Result<ResolvedFiles> {
    let mut files = Vec::new();
    let mut unmatched = Vec::new();

    for raw in raw_paths {
        if is_glob_pattern(raw) {
            let full_pattern = repo_path.join(raw).to_string_lossy().to_string();
            let mut matched = false;
            match glob::glob(&full_pattern) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        if entry.is_file() {
                            if let Ok(rel) = entry.strip_prefix(repo_path) {
                                files.push(rel.to_path_buf());
                                matched = true;
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Invalid glob pattern '{}': {}", raw, e);
                    unmatched.push(raw.clone());
                    continue;
                }
            }
            if !matched {
                unmatched.push(raw.clone());
            }
        } else {
            let full_path = repo_path.join(raw);
            if full_path.is_file() {
                if let Ok(rel) = full_path.strip_prefix(repo_path) {
                    files.push(rel.to_path_buf());
                }
            } else if full_path.is_dir() {
                collect_files_recursive(&full_path, repo_path, &mut files)?;
            } else {
                warn!("Path does not exist: {}", full_path.display());
                unmatched.push(raw.clone());
            }
        }
    }

    // Deduplicate
    files.sort();
    files.dedup();

    info!("Resolved {} files from {} path arguments", files.len(), raw_paths.len());
    Ok(ResolvedFiles { files, unmatched_patterns: unmatched })
}

/// Execute a one-shot force sync: send `files` to the remote via rsync.
pub fn force_sync(workspace_name: &str, repo_path: &Path, files: &[PathBuf]) -> Result<()> {
    let remote_path = PathBuf::from("/home/workspace/scio");
    let client = RsyncClient::new(workspace_name.to_string(), repo_path.to_path_buf(), remote_path);
    client.sync_files(files, false)
}

/// Load persisted force-watch patterns for a devdock instance.
pub fn load_force_patterns(devdock_id: &str) -> Result<ForcePatterns> {
    let path = force_patterns_path(devdock_id);
    if !path.exists() {
        return Ok(ForcePatterns::default());
    }
    let content = std::fs::read_to_string(&path)?;
    let patterns: ForcePatterns = serde_json::from_str(&content)
        .map_err(|e| SyncError::DaemonError(format!("Failed to parse force patterns: {e}")))?;
    Ok(patterns)
}

/// Save force-watch patterns for a devdock instance (merge with existing).
pub fn save_force_patterns(devdock_id: &str, new_patterns: &[String]) -> Result<()> {
    let mut existing = load_force_patterns(devdock_id)?;
    for p in new_patterns {
        if !existing.patterns.contains(p) {
            existing.patterns.push(p.clone());
        }
    }
    let path = force_patterns_path(devdock_id);
    let json = serde_json::to_string_pretty(&existing)
        .map_err(|e| SyncError::DaemonError(format!("Failed to serialize force patterns: {e}")))?;
    std::fs::write(&path, json)?;
    info!("Saved {} force-watch patterns to {}", existing.patterns.len(), path.display());
    Ok(())
}

/// Check if a relative path matches any force-watch patterns.
pub fn matches_force_pattern(path: &Path, patterns: &[String]) -> bool {
    let path_str = path.to_string_lossy();
    for pattern in patterns {
        if let Ok(glob_pattern) = glob::Pattern::new(pattern) {
            if glob_pattern.matches(&path_str) {
                return true;
            }
        }
    }
    false
}

/// Get the file path for persisted force patterns.
pub fn force_patterns_path(devdock_id: &str) -> PathBuf {
    let sync_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join(".devdock/sync");
    let _ = std::fs::create_dir_all(&sync_dir);
    sync_dir.join(format!("{devdock_id}.force_patterns.json"))
}

/// Environment variable that lets a user add force-sync patterns without touching the
/// profile YAML or running `sync force --watch`. Comma-separated globs.
pub const FORCE_PATTERNS_ENV_VAR: &str = "DEVDOCK_FORCE_SYNC_PATTERNS";

/// Parse `DEVDOCK_FORCE_SYNC_PATTERNS` (comma-separated globs) into a pattern list.
/// Empty / missing var yields an empty vec. Whitespace around entries is trimmed, and
/// empty entries (e.g. `,,`) are dropped.
pub fn patterns_from_env() -> Vec<String> {
    parse_patterns(std::env::var(FORCE_PATTERNS_ENV_VAR).ok().as_deref())
}

fn parse_patterns(raw: Option<&str>) -> Vec<String> {
    raw.map(|value| {
        value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
    })
    .unwrap_or_default()
}

/// Merge force-sync pattern sources into a deduplicated list while preserving insertion
/// order. Sources are layered: persisted file (from `sync force --watch`) first, then
/// profile-level patterns, then env-var patterns. Later duplicates are dropped.
pub fn merge_patterns(persisted: Vec<String>, profile: &[String], env: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(persisted.len() + profile.len() + env.len());
    let mut seen = std::collections::HashSet::new();
    for p in persisted.into_iter().chain(profile.iter().cloned()).chain(env.iter().cloned()) {
        if seen.insert(p.clone()) {
            out.push(p);
        }
    }
    out
}

/// Check if a string contains glob metacharacters.
fn is_glob_pattern(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[')
}

/// Recursively collect all files under `dir`, returning paths relative to `base`.
fn collect_files_recursive(dir: &Path, base: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Ok(rel) = path.strip_prefix(base) {
                files.push(rel.to_path_buf());
            }
        } else if path.is_dir() {
            // Skip .git and node_modules directories
            let name = path.file_name().map(|n| n.to_string_lossy().to_string());
            if matches!(name.as_deref(), Some(".git") | Some("node_modules")) {
                debug!("Skipping directory: {}", path.display());
                continue;
            }
            collect_files_recursive(&path, base, files)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_deduplicates_and_preserves_layer_order() {
        let persisted = vec!["a".to_string(), "b".to_string()];
        let profile = vec!["b".to_string(), "c".to_string()];
        let env = vec!["c".to_string(), "d".to_string()];

        let merged = merge_patterns(persisted, &profile, &env);

        assert_eq!(merged, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn merge_handles_all_empty_sources() {
        let merged = merge_patterns(Vec::new(), &[], &[]);
        assert!(merged.is_empty());
    }

    #[test]
    fn merge_with_only_env() {
        let merged = merge_patterns(Vec::new(), &[], &["config/**".to_string()]);
        assert_eq!(merged, vec!["config/**"]);
    }

    #[test]
    fn patterns_from_env_parses_comma_separated() {
        assert_eq!(
            parse_patterns(Some("  config/shards/** , **/BUILD.bazel ,,")),
            vec!["config/shards/**", "**/BUILD.bazel"]
        );
    }

    #[test]
    fn patterns_from_env_returns_empty_when_unset() {
        assert!(parse_patterns(None).is_empty());
    }
}
