//! Git-aware file listing.

use crate::{
    policy::{universal_matcher, SyncMatcher},
    Result, SyncError,
};
use git2::Repository;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Check if a file should be synced based on patterns (monitored dirs + extensions).
pub fn should_sync_file_by_pattern(path: &Path) -> bool {
    universal_matcher().is_syncable(path)
}

pub fn should_sync_file_by_pattern_with(matcher: &SyncMatcher, path: &Path) -> bool {
    matcher.is_syncable(path)
}

/// List all git-tracked files in monitored directories that match sync patterns.
pub fn list_git_tracked_files(repo_path: &Path) -> Result<Vec<PathBuf>> {
    list_git_tracked_files_with(repo_path, universal_matcher())
}

pub fn list_git_tracked_files_with(
    repo_path: &Path,
    matcher: &SyncMatcher,
) -> Result<Vec<PathBuf>> {
    info!("Listing git-tracked files in monitored directories");

    let repo = Repository::open(repo_path)
        .map_err(|_| SyncError::GitRepoNotFound(repo_path.display().to_string()))?;

    let mut files = Vec::new();
    let index = repo.index()?;

    for entry in index.iter() {
        let path = PathBuf::from(String::from_utf8_lossy(&entry.path).to_string());
        let full_path = repo_path.join(&path);

        if full_path.exists() && full_path.is_file() && matcher.is_syncable(&path) {
            files.push(path);
        }
    }

    info!("Found {} git-tracked files to sync (after filtering)", files.len());
    Ok(files)
}

/// List all files in a directory (respecting .gitignore).
pub fn list_all_files(repo_path: &Path) -> Result<Vec<PathBuf>> {
    debug!("Listing all files in {:?}", repo_path);

    let mut files = Vec::new();

    let walker =
        WalkBuilder::new(repo_path).hidden(false).git_ignore(true).git_exclude(true).build();

    for entry in walker.flatten() {
        if entry.path().is_file() {
            if let Ok(rel_path) = entry.path().strip_prefix(repo_path) {
                files.push(rel_path.to_path_buf());
            }
        }
    }

    debug!("Found {} files total", files.len());
    Ok(files)
}

/// List git-modified files (changed since origin/master or origin/main).
/// This is used for fast initial sync - only sync files you're actually working on.
pub fn list_git_modified_files(repo_path: &Path) -> Result<Vec<PathBuf>> {
    list_git_modified_files_with(repo_path, universal_matcher())
}

pub fn list_git_modified_files_with(
    repo_path: &Path,
    matcher: &SyncMatcher,
) -> Result<Vec<PathBuf>> {
    info!("Finding git-modified files since origin/master...");

    use std::process::Command;

    // For Scio, we always use origin/master as the base branch
    let output = Command::new("git")
        .args(["merge-base", "origin/master", "HEAD"])
        .current_dir(repo_path)
        .output();

    let base = if let Ok(output) = output {
        if output.status.success() {
            let merge_base = String::from_utf8_lossy(&output.stdout).trim().to_string();
            info!(
                "Using merge base with origin/master: {}",
                &merge_base[..8.min(merge_base.len())]
            );
            merge_base
        } else {
            warn!("Could not find merge base with origin/master, using HEAD (uncommitted changes only)");
            "HEAD".to_string()
        }
    } else {
        warn!("Git merge-base failed, using HEAD (uncommitted changes only)");
        "HEAD".to_string()
    };

    // Get changed files: modified, added, renamed
    let output = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=ACDMRT", &base])
        .current_dir(repo_path)
        .output()
        .map_err(|e| SyncError::DaemonError(format!("Failed to run git diff: {e}")))?;

    if !output.status.success() {
        warn!("git diff failed, falling back to empty list");
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files = Vec::new();

    for line in stdout.lines() {
        let path = PathBuf::from(line.trim());
        let full_path = repo_path.join(&path);

        if full_path.exists() && full_path.is_file() && matcher.is_syncable(&path) {
            files.push(path);
        }
    }

    // Also collect untracked (but not gitignored) files so newly created files
    // are included in the initial sync even before `git add`.
    let untracked_output = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(repo_path)
        .output();

    if let Ok(ut_output) = untracked_output {
        if ut_output.status.success() {
            let ut_stdout = String::from_utf8_lossy(&ut_output.stdout);
            let mut untracked_count = 0usize;

            for line in ut_stdout.lines() {
                let path = PathBuf::from(line.trim());
                let full_path = repo_path.join(&path);

                if full_path.exists()
                    && full_path.is_file()
                    && matcher.is_syncable(&path)
                    && !files.contains(&path)
                {
                    files.push(path);
                    untracked_count += 1;
                }
            }

            if untracked_count > 0 {
                info!("Found {} additional untracked files to sync", untracked_count);
            }
        } else {
            warn!("git ls-files --others failed, skipping untracked files");
        }
    }

    info!("Found {} total files to sync (modified + untracked)", files.len());
    Ok(files)
}

/// List all git-tracked files plus untracked files.
/// Used when syncing entire branch (not just diff from master).
pub fn list_git_tracked_and_untracked_files(repo_path: &Path) -> Result<Vec<PathBuf>> {
    list_git_tracked_and_untracked_files_with(repo_path, universal_matcher())
}

pub fn list_git_tracked_and_untracked_files_with(
    repo_path: &Path,
    matcher: &SyncMatcher,
) -> Result<Vec<PathBuf>> {
    info!("Listing all git-tracked and untracked files");

    // Start with all tracked files
    let mut files = list_git_tracked_files_with(repo_path, matcher)?;

    // Add untracked files (same pattern as list_git_modified_files)
    use std::process::Command;
    let untracked_output = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(repo_path)
        .output();

    if let Ok(ut_output) = untracked_output {
        if ut_output.status.success() {
            let ut_stdout = String::from_utf8_lossy(&ut_output.stdout);
            let mut untracked_count = 0usize;

            for line in ut_stdout.lines() {
                let path = PathBuf::from(line.trim());
                let full_path = repo_path.join(&path);

                if full_path.exists()
                    && full_path.is_file()
                    && matcher.is_syncable(&path)
                    && !files.contains(&path)
                {
                    files.push(path);
                    untracked_count += 1;
                }
            }

            if untracked_count > 0 {
                info!("Found {} additional untracked files to sync", untracked_count);
            }
        } else {
            warn!("git ls-files --others failed, skipping untracked files");
        }
    }

    info!("Found {} total files to sync (tracked + untracked)", files.len());
    Ok(files)
}

/// Check if a path should be synced based on git tracking.
pub fn should_sync_file(repo_path: &Path, file_path: &Path, full_mode: bool) -> Result<bool> {
    should_sync_file_with(universal_matcher(), repo_path, file_path, full_mode)
}

pub fn should_sync_file_with(
    matcher: &SyncMatcher,
    repo_path: &Path,
    file_path: &Path,
    full_mode: bool,
) -> Result<bool> {
    if full_mode {
        // In full mode, sync all files (respecting .gitignore via WalkBuilder)
        return Ok(true);
    }

    // Get relative path for checking
    let relative_path = file_path
        .strip_prefix(repo_path)
        .map_err(|_| SyncError::GitRepoNotFound("Invalid path".to_string()))?;

    // Check if it's a special file that doesn't need git tracking
    let path_str = relative_path.to_string_lossy();
    if matcher.syncs_without_git_tracking(relative_path) {
        debug!("File {} syncs without git tracking (special file)", path_str);
        return Ok(true);
    }

    // Allow files that are git-tracked OR untracked-but-not-gitignored.
    // This ensures newly created files sync before they are `git add`-ed.
    let repo = Repository::open(repo_path)
        .map_err(|_| SyncError::GitRepoNotFound(repo_path.display().to_string()))?;

    let index = repo.index()?;
    if index.get_path(relative_path, 0).is_some() {
        return Ok(true);
    }

    // Not in the index — sync it anyway unless git says to ignore it
    match repo.status_should_ignore(relative_path) {
        Ok(ignored) => {
            if ignored {
                debug!("File {} is gitignored, skipping", path_str);
            } else {
                debug!("File {} is untracked but not gitignored, will sync", path_str);
            }
            Ok(!ignored)
        }
        Err(e) => {
            debug!("Could not check ignore status for {}: {}, falling back to skip", path_str, e);
            Ok(false)
        }
    }
}
