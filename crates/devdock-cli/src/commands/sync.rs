//! File sync command implementation.

use anyhow::{anyhow, Result};
use colored::Colorize;
use devdock_state::StateManager;
use devdock_sync::daemon as sync_daemon;
use std::path::PathBuf;

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

    if let Some((workspace_name, pid, full_mode, status)) =
        state.get_sync_session(&devdock_id).await?
    {
        println!("\n{}", "Sync Status:".bold());
        println!("  Workspace: {}", workspace_name.cyan());
        println!(
            "  Status: {}",
            match status.as_str() {
                "running" => status.green(),
                "stopped" => status.yellow(),
                "error" => status.red(),
                _ => status.normal(),
            }
        );
        println!("  Mode: {}", if full_mode { "all files" } else { "git-tracked only" });

        if let Some(pid) = pid {
            println!("  PID: {}", pid);
        }
    } else {
        println!("{}", "No sync session found".yellow());
        println!("Start sync with: {}", "devdock sync start".cyan());
    }

    Ok(())
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
