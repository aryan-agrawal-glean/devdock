//! List command implementation.

use anyhow::Result;
use colored::Colorize;
use devdock_state::{Instance, InstanceStatus, StateManager};

pub async fn execute(running_only: bool) -> Result<()> {
    let state_manager = StateManager::new().await?;
    let instances = state_manager.list_instances().await?;

    // Filter if requested
    let filtered: Vec<_> = if running_only {
        instances
            .into_iter()
            .filter(|i| matches!(i.status, InstanceStatus::Running | InstanceStatus::Starting))
            .collect()
    } else {
        instances
    };

    if filtered.is_empty() {
        if running_only {
            println!("\n{}", "No running DevDock instances.".yellow());
        } else {
            println!("\n{}", "No DevDock instances found.".yellow());
        }
        println!("\nTo start DevDock:");
        println!("  devdock start <profile_name>");
        return Ok(());
    }

    println!("\n{}", "DevDock Instances".bold());
    println!("{}", "═".repeat(110));
    println!(
        "{:<10} {:<12} {:<8} {:<15} {:<8} {:<10} {:<40}",
        "ID".bold(),
        "STATUS".bold(),
        "MODE".bold(),
        "PROFILE".bold(),
        "PORT".bold(),
        "BRANCH".bold(),
        "WORKSPACE".bold()
    );
    println!("{}", "─".repeat(110));

    for instance in &filtered {
        let status_str = match instance.status {
            InstanceStatus::Running => "● Running".green().to_string(),
            InstanceStatus::Starting => "◐ Starting".yellow().to_string(),
            InstanceStatus::Stopping => "◑ Stopping".yellow().to_string(),
            InstanceStatus::Stopped => "○ Stopped".dimmed().to_string(),
            InstanceStatus::Error => "✗ Error".red().to_string(),
            InstanceStatus::Quarantined => "⚠ Quarantined".yellow().to_string(),
        };

        // Display mode
        let mode = if instance.mode == "remote" { "Remote" } else { "Local" };

        // For remote instances, show workspace_name if available, otherwise show workspace_path
        // For local instances, always show workspace_path
        let workspace = if instance.mode == "remote" {
            if let Some(ref name) = instance.workspace_name {
                if name.len() > 38 {
                    format!("...{}", &name[name.len() - 35..])
                } else {
                    name.clone()
                }
            } else {
                // Fallback to path if workspace_name is missing
                if instance.workspace_path.len() > 38 {
                    format!("...{}", &instance.workspace_path[instance.workspace_path.len() - 35..])
                } else {
                    instance.workspace_path.clone()
                }
            }
        } else {
            // Local instance - show workspace path
            if instance.workspace_path.len() > 38 {
                format!("...{}", &instance.workspace_path[instance.workspace_path.len() - 35..])
            } else {
                instance.workspace_path.clone()
            }
        };

        // Truncate branch if too long
        let branch = if instance.branch.len() > 8 {
            format!("{}...", &instance.branch[..5])
        } else {
            instance.branch.clone()
        };

        println!(
            "{:<10} {:<12} {:<8} {:<15} {:<8} {:<10} {:<40}",
            Instance::display_id(&instance.id),
            status_str,
            mode,
            instance.profile,
            instance.port_base,
            branch,
            workspace
        );
    }

    println!("{}", "─".repeat(110));
    println!("Total: {} instance{}", filtered.len(), if filtered.len() == 1 { "" } else { "s" });

    // Show hint for more details
    println!("\nFor details on a specific instance:");
    println!("  devdock status --detailed");

    Ok(())
}
