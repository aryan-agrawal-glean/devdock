//! File sync command implementation.

use anyhow::{anyhow, Result};
use colored::Colorize;
use devdock_state::StateManager;
use devdock_sync::daemon as sync_daemon;
use devdock_sync::stats;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct QueueData {
    pending: Vec<String>,
    in_progress: Vec<String>,
    #[allow(dead_code)]
    last_update: String,
}

/// Start file sync.
pub async fn start(full: bool) -> Result<()> {
    let state = StateManager::new().await?;

    // Get current instance
    let (devdock_id, workspace_name) = get_current_instance(&state).await?;
    let worktree = get_current_worktree()?;

    println!("Starting file sync...");
    println!("  Mode: {}", if full { "all files" } else { "git-tracked only" });

    let repo_path = PathBuf::from(&worktree);
    let _pid = sync_daemon::start_daemon(&devdock_id, &workspace_name, &repo_path, full)?;

    // Update database
    state.save_sync_session(&devdock_id, &workspace_name, None, full, "running").await?;

    println!("{}", "✅ File sync started".green());
    println!("  Check status: {}", "devdock sync status".cyan());
    println!("  Stop sync: {}", "devdock sync stop".cyan());

    Ok(())
}

/// Stop file sync.
pub async fn stop() -> Result<()> {
    let state = StateManager::new().await?;

    let (devdock_id, _) = get_current_instance(&state).await?;

    println!("Stopping file sync...");
    sync_daemon::stop_daemon(&devdock_id)?;

    // Update database
    state.save_sync_session(&devdock_id, "", None, false, "stopped").await?;

    println!("{}", "✅ File sync stopped".green());

    Ok(())
}

/// Show file sync status.
pub async fn status() -> Result<()> {
    let state = StateManager::new().await?;

    let (devdock_id, _) = get_current_instance(&state).await?;
    let worktree = get_current_worktree()?;

    if let Some((workspace_name, pid, full_mode, db_status)) =
        state.get_sync_session(&devdock_id).await?
    {
        // Try to read detailed stats
        match stats::read_session_stats(&devdock_id) {
            Ok(session_stats) => {
                // Enhanced status display with stats
                println!();
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!(
                    "{}: {} {}",
                    "Sync Status".bold(),
                    match db_status.as_str() {
                        "running" => "🟢",
                        "stopped" => "🟡",
                        "error" => "🔴",
                        _ => "⚪",
                    },
                    db_status.to_uppercase().bold()
                );
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();
                println!("Workspace:    {}", workspace_name.cyan());
                println!(
                    "Mode:         {}",
                    if full_mode {
                        "all files".yellow()
                    } else {
                        "git-tracked (fast startup)".cyan()
                    }
                );
                if let Some(pid) = pid {
                    println!("PID:          {}", pid.to_string().dimmed());
                }

                // Parse uptime from session_start
                if let Ok(start_time) =
                    chrono::DateTime::parse_from_rfc3339(&session_stats.session_start)
                {
                    let now = chrono::Utc::now();
                    let duration =
                        now.signed_duration_since(start_time.with_timezone(&chrono::Utc));
                    let hours = duration.num_hours();
                    let minutes = duration.num_minutes() % 60;
                    println!("Uptime:       {}h {}m", hours, minutes);
                }

                println!();
                println!("{}", "Session Metrics:".bold());

                let total_files = session_stats.total_files_synced;
                let total_bytes = session_stats.total_bytes_synced;
                let total_syncs = session_stats.successful_syncs + session_stats.failed_syncs;

                println!(
                    "  Files synced:   {} files ({})",
                    total_files.to_string().green(),
                    format_bytes(total_bytes).green()
                );

                if total_syncs > 0 {
                    let success_rate =
                        (session_stats.successful_syncs as f64 / total_syncs as f64) * 100.0;
                    println!(
                        "  Success rate:   {:.1}% ({} syncs, {} failures)",
                        success_rate,
                        session_stats.successful_syncs.to_string().green(),
                        session_stats.failed_syncs.to_string().red()
                    );
                }

                if let Some(last_sync) = &session_stats.last_sync {
                    if let Ok(last_sync_time) = chrono::DateTime::parse_from_rfc3339(last_sync) {
                        let now = chrono::Utc::now();
                        let elapsed =
                            now.signed_duration_since(last_sync_time.with_timezone(&chrono::Utc));

                        let elapsed_str = if elapsed.num_seconds() < 60 {
                            format!("{} seconds ago", elapsed.num_seconds())
                        } else if elapsed.num_minutes() < 60 {
                            format!("{} minutes ago", elapsed.num_minutes())
                        } else {
                            format!("{}h {}m ago", elapsed.num_hours(), elapsed.num_minutes() % 60)
                        };

                        println!("  Last sync:      {}", elapsed_str.dimmed());
                    }
                }

                // Show pending/in-progress files from queue
                if let Ok(queue_data) = read_queue_file(&devdock_id) {
                    if !queue_data.in_progress.is_empty() {
                        println!();
                        println!("{}", "Currently Syncing:".yellow().bold());
                        for file in queue_data.in_progress.iter().take(10) {
                            println!("  ⟳  {}", file.yellow());
                        }
                        if queue_data.in_progress.len() > 10 {
                            println!("  ... and {} more", queue_data.in_progress.len() - 10);
                        }
                    }

                    if !queue_data.pending.is_empty() {
                        println!();
                        println!("{}", "Pending Files:".cyan().bold());
                        for file in queue_data.pending.iter().take(10) {
                            println!("  ○  {}", file.dimmed());
                        }
                        if queue_data.pending.len() > 10 {
                            println!("  ... and {} more", queue_data.pending.len() - 10);
                        }
                    }
                }

                // If no pending files, check for git-modified files that haven't been detected yet
                if db_status == "running" {
                    if let Ok(queue_data) = read_queue_file(&devdock_id) {
                        if queue_data.pending.is_empty() && queue_data.in_progress.is_empty() {
                            // Check for git-modified files
                            use std::path::PathBuf;
                            let repo_path = PathBuf::from(&worktree);
                            if let Ok(modified_files) =
                                devdock_sync::git::list_git_modified_files(&repo_path)
                            {
                                if !modified_files.is_empty() {
                                    println!();
                                    println!(
                                        "{}",
                                        "Git-Modified Files (not yet detected by watcher):"
                                            .dimmed()
                                    );
                                    for file in modified_files.iter().take(10) {
                                        println!("  ○  {}", file.display().to_string().dimmed());
                                    }
                                    if modified_files.len() > 10 {
                                        println!("  ... and {} more", modified_files.len() - 10);
                                    }
                                    println!();
                                    println!(
                                        "  {}",
                                        "These will sync when modified or on next file save"
                                            .dimmed()
                                    );
                                }
                            }
                        }
                    }
                }

                // Show recent activity from history
                if let Ok(history) = stats::read_history_entries(&devdock_id, Some(5)) {
                    if !history.is_empty() {
                        println!();
                        println!("{}", "Recent Activity:".bold());
                        for entry in history.iter().rev() {
                            let status_icon = if entry.ok { "✓".green() } else { "✗".red() };
                            let timestamp = entry.ts.split('T').nth(1).unwrap_or("--:--:--");
                            let time = &timestamp[0..8]; // HH:MM:SS

                            let files_display = if entry.count <= 3 {
                                entry.files.join(", ")
                            } else {
                                format!("{} files in batch", entry.count)
                            };

                            println!(
                                "  {}  {} {} [{}ms]",
                                time.dimmed(),
                                status_icon,
                                files_display,
                                entry.dur_ms
                            );
                        }
                    }
                }

                // Show recent errors if any
                if session_stats.failed_syncs > 0 {
                    if let Ok(errors) = stats::read_error_entries(&devdock_id, Some(3)) {
                        if !errors.is_empty() {
                            println!();
                            println!("{}", "Recent Errors:".red().bold());
                            for error in errors.iter().rev().take(3) {
                                let timestamp = error.ts.split('T').nth(1).unwrap_or("--:--:--");
                                let time = &timestamp[0..8];
                                println!(
                                    "  {}  {} - {}",
                                    time.dimmed(),
                                    error.files.join(", "),
                                    error.error.red()
                                );
                            }
                        }
                    }
                }

                println!();
                println!("{}", "Commands:".dimmed());
                println!("  {}  Show detailed sync history", "devdock sync history".cyan());
                println!("  {}       Show recent errors", "devdock sync errors".cyan());
                println!("  {}        Stop file sync", "devdock sync stop".cyan());
                println!();
            }
            Err(_) => {
                // Fallback to basic status if stats file doesn't exist yet
                println!();
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!(
                    "{}: {} {}",
                    "Sync Status".bold(),
                    match db_status.as_str() {
                        "running" => "🟢",
                        "stopped" => "🟡",
                        "error" => "🔴",
                        _ => "⚪",
                    },
                    db_status.to_uppercase().bold()
                );
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();
                println!("Workspace:    {}", workspace_name.cyan());
                println!(
                    "Mode:         {}",
                    if full_mode {
                        "all files".yellow()
                    } else {
                        "git-tracked (fast startup)".cyan()
                    }
                );
                if let Some(pid) = pid {
                    println!("PID:          {}", pid.to_string().dimmed());
                }
                println!();

                // Show pending/in-progress even if no history yet
                if let Ok(queue_data) = read_queue_file(&devdock_id) {
                    if !queue_data.in_progress.is_empty() {
                        println!("{}", "Currently Syncing:".yellow().bold());
                        for file in queue_data.in_progress.iter().take(10) {
                            println!("  ⟳  {}", file.yellow());
                        }
                        if queue_data.in_progress.len() > 10 {
                            println!("  ... and {} more", queue_data.in_progress.len() - 10);
                        }
                        println!();
                    }

                    if !queue_data.pending.is_empty() {
                        println!("{}", "Pending Files:".cyan().bold());
                        for file in queue_data.pending.iter().take(10) {
                            println!("  ○  {}", file.dimmed());
                        }
                        if queue_data.pending.len() > 10 {
                            println!("  ... and {} more", queue_data.pending.len() - 10);
                        }
                        println!();
                    }

                    // If no queue, show git-modified files
                    if queue_data.pending.is_empty()
                        && queue_data.in_progress.is_empty()
                        && db_status == "running"
                    {
                        use std::path::PathBuf;
                        let repo_path = PathBuf::from(&worktree);
                        if let Ok(modified_files) =
                            devdock_sync::list_git_modified_files(&repo_path)
                        {
                            if !modified_files.is_empty() {
                                println!(
                                    "{}",
                                    "Git-Modified Files (will sync on next change):".dimmed()
                                );
                                for file in modified_files.iter().take(10) {
                                    println!("  ○  {}", file.display().to_string().dimmed());
                                }
                                if modified_files.len() > 10 {
                                    println!("  ... and {} more", modified_files.len() - 10);
                                }
                                println!();
                            } else {
                                println!(
                                    "{}",
                                    "✓ No pending files - everything is in sync!".green()
                                );
                                println!();
                            }
                        }
                    }
                } else {
                    // No queue file, check git-modified
                    use std::path::PathBuf;
                    let repo_path = PathBuf::from(&worktree);
                    if let Ok(modified_files) = devdock_sync::list_git_modified_files(&repo_path) {
                        if !modified_files.is_empty() {
                            println!(
                                "{}",
                                "Git-Modified Files (will sync on next change):".dimmed()
                            );
                            for file in modified_files.iter().take(10) {
                                println!("  ○  {}", file.display().to_string().dimmed());
                            }
                            if modified_files.len() > 10 {
                                println!("  ... and {} more", modified_files.len() - 10);
                            }
                            println!();
                        } else {
                            println!("{}", "✓ No pending files - everything is in sync!".green());
                            println!();
                        }
                    }
                }

                println!(
                    "{}",
                    "Note: Detailed stats will be available after first sync operation".dimmed()
                );
                println!();
            }
        }
    } else {
        println!("{}", "No sync session found".yellow());
        println!("Start sync with: {}", "devdock sync start".cyan());
    }

    Ok(())
}

/// Format bytes in human-readable format
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

/// Read queue file
fn read_queue_file(devdock_id: &str) -> Result<QueueData> {
    let sync_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join(".devdock/sync");
    let queue_file = sync_dir.join(format!("{}.queue.json", devdock_id));

    if !queue_file.exists() {
        return Ok(QueueData {
            pending: Vec::new(),
            in_progress: Vec::new(),
            last_update: String::new(),
        });
    }

    let content = std::fs::read_to_string(&queue_file)?;
    let queue: QueueData = serde_json::from_str(&content)?;
    Ok(queue)
}

/// Get current DevDock instance.
async fn get_current_instance(state: &StateManager) -> Result<(String, String)> {
    let worktree = get_current_worktree()?;
    let instances = state.list_instances().await?;

    let instance = instances
        .into_iter()
        .find(|i| i.workspace_path == worktree && i.status.as_str() == "running")
        .ok_or_else(|| anyhow!("No running DevDock instance found for current worktree"))?;

    // Get workspace name from instance (for remote instances, use coder workspace name)
    let workspace_name = instance.workspace_name.clone().unwrap_or_else(|| instance.id.clone());

    Ok((instance.id.clone(), workspace_name))
}

/// Get current git worktree path.
fn get_current_worktree() -> Result<String> {
    use std::process::Command;

    let output = Command::new("git").args(["rev-parse", "--show-toplevel"]).output()?;

    if !output.status.success() {
        return Err(anyhow!("Not in a git repository"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Show sync history.
pub async fn history(tail: Option<usize>) -> Result<()> {
    let state = StateManager::new().await?;
    let (devdock_id, _) = get_current_instance(&state).await?;

    let entries = stats::read_history_entries(&devdock_id, tail)?;

    if entries.is_empty() {
        println!("{}", "No sync history found".yellow());
        println!("History will be available after syncing files.");
        return Ok(());
    }

    println!();
    println!("{}", "Sync History:".bold());
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    for entry in entries.iter().rev() {
        let status_icon = if entry.ok { "✓".green() } else { "✗".red() };
        let timestamp = entry.ts.split('T').nth(1).unwrap_or("--:--:--");
        let time = &timestamp[0..8]; // HH:MM:SS

        println!(
            "{}  {} {} file(s) in {}ms ({})",
            time.dimmed(),
            status_icon,
            entry.count.to_string().cyan(),
            entry.dur_ms,
            format_bytes(entry.bytes).dimmed()
        );

        // Show file list (up to 10 files)
        for (i, file) in entry.files.iter().enumerate() {
            if i >= 10 {
                println!("         ... and {} more", entry.files.len() - 10);
                break;
            }
            println!("         • {}", file.dimmed());
        }
        println!();
    }

    let showing = entries.len();
    let tail_info = tail.map(|n| format!(" (showing last {})", n)).unwrap_or_default();
    println!("{}", format!("Showing {} entries{}", showing, tail_info).dimmed());
    println!();

    Ok(())
}

/// Show sync errors.
pub async fn errors(tail: Option<usize>) -> Result<()> {
    let state = StateManager::new().await?;
    let (devdock_id, _) = get_current_instance(&state).await?;

    let entries = stats::read_error_entries(&devdock_id, tail)?;

    if entries.is_empty() {
        println!("{}", "✓ No sync errors found".green());
        return Ok(());
    }

    println!();
    println!("{}", "Sync Errors:".red().bold());
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    for entry in entries.iter().rev() {
        let timestamp = entry.ts.split('T').nth(1).unwrap_or("--:--:--");
        let time = &timestamp[0..8]; // HH:MM:SS

        println!("{}  {}", time.dimmed(), "✗ Error".red().bold());
        println!("  Files:");
        for file in &entry.files {
            println!("    • {}", file.yellow());
        }
        println!("  Error: {}", entry.error.red());
        println!();
    }

    let showing = entries.len();
    let tail_info = tail.map(|n| format!(" (showing last {})", n)).unwrap_or_default();
    println!("{}", format!("Showing {} errors{}", showing, tail_info).dimmed());
    println!();
    println!("{}", "Tip: Check logs for more details:".dimmed());
    println!("  {}", format!("less ~/.devdock/sync/{}.log", devdock_id).cyan());
    println!();

    Ok(())
}
