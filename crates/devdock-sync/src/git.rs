//! Git-aware file listing.

use crate::{Result, SyncError};
use git2::Repository;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use tracing::debug;

/// List all git-tracked files in a repository.
pub fn list_git_tracked_files(repo_path: &Path) -> Result<Vec<PathBuf>> {
    debug!("Listing git-tracked files in {:?}", repo_path);
    
    let repo = Repository::open(repo_path)
        .map_err(|_| SyncError::GitRepoNotFound(repo_path.display().to_string()))?;
    
    let mut files = Vec::new();
    let index = repo.index()?;
    
    for entry in index.iter() {
        let path = PathBuf::from(String::from_utf8_lossy(&entry.path).to_string());
        let full_path = repo_path.join(&path);
        
        if full_path.exists() && full_path.is_file() {
            files.push(path);
        }
    }
    
    debug!("Found {} git-tracked files", files.len());
    Ok(files)
}

/// List all files in a directory (respecting .gitignore).
pub fn list_all_files(repo_path: &Path) -> Result<Vec<PathBuf>> {
    debug!("Listing all files in {:?}", repo_path);
    
    let mut files = Vec::new();
    
    let walker = WalkBuilder::new(repo_path)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .build();
    
    for entry in walker {
        if let Ok(entry) = entry {
            if entry.path().is_file() {
                if let Ok(rel_path) = entry.path().strip_prefix(repo_path) {
                    files.push(rel_path.to_path_buf());
                }
            }
        }
    }
    
    debug!("Found {} files total", files.len());
    Ok(files)
}

/// Check if a path should be synced based on git tracking.
pub fn should_sync_file(repo_path: &Path, file_path: &Path, full_mode: bool) -> Result<bool> {
    if full_mode {
        // In full mode, sync all files (respecting .gitignore via WalkBuilder)
        Ok(true)
    } else {
        // In git mode, only sync tracked files
        let repo = Repository::open(repo_path)
            .map_err(|_| SyncError::GitRepoNotFound(repo_path.display().to_string()))?;
        
        let index = repo.index()?;
        let relative_path = file_path.strip_prefix(repo_path)
            .map_err(|_| SyncError::GitRepoNotFound("Invalid path".to_string()))?;
        
        Ok(index.get_path(relative_path, 0).is_some())
    }
}

