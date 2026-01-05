//! Port mapping display command.

use anyhow::{anyhow, Result};
use colored::Colorize;
use devdock_state::StateManager;
use prettytable::{format, row, Table};

/// Execute the ports command.
pub async fn execute(instance: Option<String>) -> Result<()> {
    let state = StateManager::new().await?;
    
    // Determine which instance to show ports for
    let devdock_id = if let Some(id) = instance {
        id
    } else {
        // Get current worktree's instance
        let worktree = get_current_worktree()?;
        let instances = state.list_instances().await?;
        
        instances
            .into_iter()
            .find(|i| i.workspace_path == worktree && i.status.as_str() == "running")
            .map(|i| i.id)
            .ok_or_else(|| anyhow!("No running DevDock instance found for current worktree"))?
    };
    
    // Get port mappings
    let mappings = state.get_port_mappings(&devdock_id).await?;
    
    if mappings.is_empty() {
        println!("{}", "No port mappings found".yellow());
        println!("Tip: Use --auto-forward when starting remote DevDock to enable auto port forwarding");
        return Ok(());
    }
    
    // Display port table
    print_port_table(&mappings);
    
    // Display service URLs
    println!("\n{}:", "Access services".bold());
    for (service_name, port_type, local_port, _remote_port, _pid) in &mappings {
        if port_type == "service" {
            println!(
                "  {} {}",
                service_name.cyan(),
                format!("http://localhost:{}", local_port).green()
            );
        }
    }
    
    Ok(())
}

/// Print port mappings table.
fn print_port_table(mappings: &[(String, String, u16, u16, Option<u32>)]) {
    let mut table = Table::new();
    table.set_format(*format::consts::FORMAT_BOX_CHARS);
    
    // Add header row
    table.add_row(row!["Service", "Type", "Local Port", "Remote Port", "Status"]);
    
    for (service_name, port_type, local_port, remote_port, pid) in mappings {
        let status = if pid.is_some() {
            "Active".green().to_string()
        } else {
            "Inactive".red().to_string()
        };
        
        table.add_row(row![
            service_name,
            port_type,
            local_port.to_string(),
            remote_port.to_string(),
            status
        ]);
    }
    
    table.printstd();
}

/// Get current git worktree path.
fn get_current_worktree() -> Result<String> {
    use std::process::Command;
    
    let output = Command::new("git")
        .args(&["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| anyhow!("Failed to get git worktree: {}", e))?;
    
    if !output.status.success() {
        return Err(anyhow!("Not in a git repository"));
    }
    
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

