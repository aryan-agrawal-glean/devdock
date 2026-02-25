//! Status command implementation.

use crate::output;
use anyhow::Result;
use colored::Colorize;
use devdock_state::{Instance, InstanceStatus, StateManager};
use prettytable::{format, row, Table};
use std::process::Command;

pub async fn execute(detailed: bool) -> Result<()> {
    output::info("Checking DevDock status...");

    let state_manager = StateManager::new().await?;
    let workspace_path = std::env::current_dir()?.display().to_string();

    // Try to find instance for current workspace
    let instance = state_manager.get_instance_by_workspace(&workspace_path).await?;

    if let Some(instance) = instance {
        print_instance_status(&state_manager, &instance, detailed).await?;
    } else {
        // Check if there are any running instances
        let all_instances = state_manager.list_instances().await?;
        let running: Vec<_> = all_instances
            .iter()
            .filter(|i| matches!(i.status, InstanceStatus::Running | InstanceStatus::Starting))
            .collect();

        if running.is_empty() {
            println!("\n{}", "No DevDock instance running for this workspace.".yellow());
            println!("\nTo start DevDock:");
            println!("  devdock start <profile_name>");
        } else {
            println!(
                "\n{} No instance for current workspace, but {} other instance(s) running:",
                "Note:".yellow(),
                running.len()
            );
            for inst in running {
                println!(
                    "  • {} ({}) - {}",
                    Instance::display_id(&inst.id),
                    inst.branch,
                    inst.workspace_path
                );
            }
        }
    }

    Ok(())
}

async fn print_instance_status(
    state_manager: &StateManager,
    instance: &Instance,
    detailed: bool,
) -> Result<()> {
    let status_str = match instance.status {
        InstanceStatus::Running => "● Running".green(),
        InstanceStatus::Starting => "◐ Starting".yellow(),
        InstanceStatus::Stopping => "◑ Stopping".yellow(),
        InstanceStatus::Stopped => "○ Stopped".red(),
        InstanceStatus::Error => "✗ Error".red(),
    };

    println!("\n{}", "DevDock Status".bold());
    println!("{}", "═".repeat(50));
    println!("  Status:     {status_str}");
    println!("  Instance:   {}", Instance::display_id(&instance.id));
    println!("  Profile:    {}", instance.profile);
    println!("  Branch:     {}", instance.branch);
    println!("  Port Base:  {}", instance.port_base);
    println!("  Mode:       {}", instance.mode);

    if detailed {
        println!("\n{}", "Details".bold());
        println!("{}", "─".repeat(50));
        println!("  Workspace:  {}", instance.workspace_path);
        println!("  Full ID:    {}", instance.id);
        println!("  Created:    {}", format_timestamp(instance.created_at));
        println!("  Updated:    {}", format_timestamp(instance.updated_at));

        // Check Docker containers (only for local mode)
        if instance.mode == "local" {
            println!("\n{}", "Docker Containers".bold());
            println!("{}", "─".repeat(50));
            print_docker_status()?;
        }

        // Check Tilt status
        println!("\n{}", "Tilt Status".bold());
        println!("{}", "─".repeat(50));
        print_tilt_status(&instance.mode, state_manager).await?;
    }

    // Always show quick access info
    println!("\n{}", "Quick Access".bold());
    println!("{}", "─".repeat(50));
    println!("  Tilt UI:    {}", format!("http://localhost:{}", instance.port_base + 10350).cyan());
    println!("  Services:   {}", format!("https://local.glean.com:{}", instance.port_base).cyan());

    // Show port mappings for remote instances
    if instance.mode == "remote" {
        let mappings = state_manager.get_port_mappings(&instance.id).await?;

        if !mappings.is_empty() {
            println!("\n{}", "Port Mappings".bold());
            println!("{}", "─".repeat(50));

            let mut table = Table::new();
            table.set_format(*format::consts::FORMAT_CLEAN);

            // Add header row
            table.add_row(row!["Service", "Type", "Local", "Remote", "Status"]);

            for (service_name, port_type, local_port, remote_port, pid) in &mappings {
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

            println!("\n  Run {} to see all port mappings", "devdock ports".cyan());
        }
    }

    Ok(())
}

fn print_docker_status() -> Result<()> {
    let output = Command::new("docker")
        .args([
            "ps",
            "--filter",
            "name=devdock-",
            "--format",
            "table {{.Names}}\t{{.Status}}\t{{.Ports}}",
        ])
        .output()?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();

        if lines.len() <= 1 {
            println!("  {}", "No DevDock containers running".yellow());
        } else {
            for line in lines {
                println!("  {line}");
            }
        }
    } else {
        println!("  {}", "Unable to query Docker".red());
    }

    Ok(())
}

async fn print_tilt_status(mode: &str, state_manager: &StateManager) -> Result<()> {
    if mode == "remote" {
        // For remote mode, check Tilt on the remote workspace
        let workspace_path = std::env::current_dir()?.display().to_string();

        if let Ok(Some(instance)) = state_manager.get_instance_by_workspace(&workspace_path).await {
            if let Ok(Some((workspace_name, _, _, _))) =
                state_manager.get_sync_session(&instance.id).await
            {
                // Check Tilt on remote using lsof
                let check_cmd = format!("coder ssh {workspace_name} -- 'lsof -i:10350 >/dev/null 2>&1 && echo running || echo stopped'");

                if let Ok(output) = Command::new("bash").args(["-c", &check_cmd]).output() {
                    let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if result == "running" {
                        println!("  {} (on remote workspace)", "Tilt is running".green());
                        return Ok(());
                    }
                }
            }
        }

        println!("  {} (on remote workspace)", "Tilt status unknown".yellow());
    } else {
        // Local mode - check local process
        let output = Command::new("pgrep").args(["-f", "tilt up"]).output();

        match output {
            Ok(output) if output.status.success() => {
                let pids = String::from_utf8_lossy(&output.stdout);
                let pid_count = pids.lines().filter(|s| !s.is_empty()).count();
                println!(
                    "  {} ({} process{})",
                    "Tilt is running".green(),
                    pid_count,
                    if pid_count == 1 { "" } else { "es" }
                );
            }
            _ => {
                println!("  {}", "Tilt is not running".yellow());
            }
        }
    }

    Ok(())
}

fn format_timestamp(ts: i64) -> String {
    use std::time::{Duration, UNIX_EPOCH};

    let datetime = UNIX_EPOCH + Duration::from_secs(ts as u64);
    let now = std::time::SystemTime::now();

    match now.duration_since(datetime) {
        Ok(elapsed) => {
            let secs = elapsed.as_secs();
            if secs < 60 {
                format!("{secs} seconds ago")
            } else if secs < 3600 {
                format!("{} minutes ago", secs / 60)
            } else if secs < 86400 {
                format!("{} hours ago", secs / 3600)
            } else {
                format!("{} days ago", secs / 86400)
            }
        }
        Err(_) => "unknown".to_string(),
    }
}
