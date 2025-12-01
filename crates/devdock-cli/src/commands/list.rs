//! List command implementation.

use anyhow::Result;
use colored::Colorize;
use devdock_state::{InstanceStatus, StateManager};

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
    println!("{}", "═".repeat(100));
    println!(
        "{:<10} {:<12} {:<15} {:<8} {:<10} {:<40}",
        "ID".bold(),
        "STATUS".bold(),
        "PROFILE".bold(),
        "PORT".bold(),
        "BRANCH".bold(),
        "WORKSPACE".bold()
    );
    println!("{}", "─".repeat(100));

    for instance in &filtered {
        let status_str = match instance.status {
            InstanceStatus::Running => "● Running".green().to_string(),
            InstanceStatus::Starting => "◐ Starting".yellow().to_string(),
            InstanceStatus::Stopping => "◑ Stopping".yellow().to_string(),
            InstanceStatus::Stopped => "○ Stopped".dimmed().to_string(),
            InstanceStatus::Error => "✗ Error".red().to_string(),
        };

        // Truncate workspace path if too long
        let workspace = if instance.workspace_path.len() > 38 {
            format!("...{}", &instance.workspace_path[instance.workspace_path.len() - 35..])
        } else {
            instance.workspace_path.clone()
        };

        // Truncate branch if too long
        let branch = if instance.branch.len() > 8 {
            format!("{}...", &instance.branch[..5])
        } else {
            instance.branch.clone()
        };

        println!(
            "{:<10} {:<12} {:<15} {:<8} {:<10} {:<40}",
            &instance.id[..8],
            status_str,
            instance.profile,
            instance.port_base,
            branch,
            workspace
        );
    }

    println!("{}", "─".repeat(100));
    println!(
        "Total: {} instance{}",
        filtered.len(),
        if filtered.len() == 1 { "" } else { "s" }
    );

    // Show hint for more details
    println!("\nFor details on a specific instance:");
    println!("  devdock status --detailed");

    Ok(())
}
