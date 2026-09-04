//! Status command implementation.

use crate::output;
use anyhow::Result;
use colored::Colorize;
use devdock_monitor::{ResourceStatus, TiltClient, TiltResource, DEFAULT_TILT_ENDPOINT};
use devdock_remote::{
    managed_port_forward_status, probe_remote_tilt, ManagedForwardStatus, RemoteTiltProbe,
    REMOTE_STATUS_SSH_TIMEOUT_SECS,
};
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
        InstanceStatus::Quarantined => "⚠ Quarantined".yellow(),
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
        if let Some(workspace_id) = &instance.workspace_id {
            println!("  Workspace ID: {workspace_id}");
        }
        if let Some(failure_stage) = &instance.failure_stage {
            println!("  Failed stage: {failure_stage}");
        }
        if let Some(failure_reason) = &instance.failure_reason {
            println!("  Failure:    {failure_reason}");
        }

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

        if instance.mode == "local" {
            println!("\n{}", "Tilt Resources".bold());
            println!("{}", "─".repeat(50));
            print_tilt_resources();
        }
    }

    // Always show quick access info
    println!("\n{}", "Quick Access".bold());
    println!("{}", "─".repeat(50));
    println!("  Tilt UI:    {}", format!("http://localhost:{}", instance.port_base + 10350).cyan());
    println!("  Services:   {}", format!("https://local.glean.com:{}", instance.port_base).cyan());

    // Show port mappings for remote instances
    if instance.mode == "remote" {
        let mappings = state_manager.get_port_mapping_records(&instance.id).await?;

        if !mappings.is_empty() {
            println!("\n{}", "Port Mappings".bold());
            println!("{}", "─".repeat(50));

            let mut table = Table::new();
            table.set_format(*format::consts::FORMAT_CLEAN);

            // Add header row
            table.add_row(row!["Service", "Type", "Local", "Remote", "Status"]);

            for mapping in &mappings {
                let status =
                    match (instance.workspace_id.as_deref(), instance.workspace_name.as_deref()) {
                        (Some(workspace_id), Some(workspace_name)) => {
                            match managed_port_forward_status(mapping, workspace_id, workspace_name)
                            {
                                ManagedForwardStatus::Active => "Active".green().to_string(),
                                ManagedForwardStatus::Exited => "Exited".red().to_string(),
                                ManagedForwardStatus::Unverified(_) => {
                                    "External/unverified".yellow().to_string()
                                }
                            }
                        }
                        _ => "External/unverified".yellow().to_string(),
                    };

                table.add_row(row![
                    mapping.service_name,
                    mapping.port_type,
                    mapping.local_port.to_string(),
                    mapping.remote_port.to_string(),
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

fn print_tilt_resources() {
    let client = TiltClient::new(DEFAULT_TILT_ENDPOINT).expect("valid default Tilt endpoint");

    match client.get_resources() {
        Ok(resources) => {
            let monitored: Vec<_> =
                resources.into_iter().filter(TiltResource::should_monitor).collect();
            if monitored.is_empty() {
                println!("  {}", "No monitored Tilt resources found".yellow());
                return;
            }

            print_resource_summary(&monitored);
            print_resource_table(&monitored);
        }
        Err(e) => {
            println!("  {} {}", "Unable to query Tilt resources:".yellow(), e);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ResourceCounts {
    healthy: usize,
    pending: usize,
    failed: usize,
    disabled: usize,
    total: usize,
}

fn count_resources(resources: &[TiltResource]) -> ResourceCounts {
    ResourceCounts {
        healthy: resources.iter().filter(|r| r.status == ResourceStatus::Healthy).count(),
        pending: resources.iter().filter(|r| r.status == ResourceStatus::Pending).count(),
        failed: resources.iter().filter(|r| r.status == ResourceStatus::Failed).count(),
        disabled: resources.iter().filter(|r| r.status == ResourceStatus::Disabled).count(),
        total: resources.len(),
    }
}

fn print_resource_summary(resources: &[TiltResource]) {
    let counts = count_resources(resources);
    println!(
        "  Summary: {} healthy, {} pending, {} failed, {} disabled / {} total",
        counts.healthy.to_string().green(),
        counts.pending.to_string().yellow(),
        counts.failed.to_string().red(),
        counts.disabled.to_string().dimmed(),
        counts.total
    );
}

fn resource_row_values(resource: &TiltResource) -> [String; 6] {
    [
        resource.name.clone(),
        format!("{:?}", resource.resource_type),
        format_resource_status(&resource.status),
        resource.runtime_status.clone().unwrap_or_else(|| "-".to_string()),
        resource.update_status.clone().unwrap_or_else(|| "-".to_string()),
        resource.compose_health.clone().unwrap_or_else(|| "-".to_string()),
    ]
}

fn print_resource_table(resources: &[TiltResource]) {
    let mut table = Table::new();
    table.set_format(*format::consts::FORMAT_CLEAN);
    table.add_row(row!["Resource", "Type", "Status", "Runtime", "Update", "Health"]);

    for resource in resources {
        let [name, resource_type, status, runtime, update, health] = resource_row_values(resource);
        table.add_row(row![name, resource_type, status, runtime, update, health]);
    }

    table.printstd();

    let first_problem = resources
        .iter()
        .find(|r| matches!(r.status, ResourceStatus::Failed | ResourceStatus::Pending));
    if let Some(resource) = first_problem {
        println!();
        println!("  {} {}", "Investigate:".dimmed(), format!("tilt logs {}", resource.name).cyan());
    }
}

fn format_resource_status(status: &ResourceStatus) -> String {
    match status {
        ResourceStatus::Healthy => "Healthy".green().to_string(),
        ResourceStatus::Pending => "Pending".yellow().to_string(),
        ResourceStatus::Failed => "Failed".red().to_string(),
        ResourceStatus::Disabled => "Disabled".dimmed().to_string(),
        ResourceStatus::Unknown => "Unknown".dimmed().to_string(),
    }
}

async fn print_tilt_status(mode: &str, state_manager: &StateManager) -> Result<()> {
    if mode == "remote" {
        let workspace_path = std::env::current_dir()?.display().to_string();

        if let Ok(Some(instance)) = state_manager.get_instance_by_workspace(&workspace_path).await {
            let workspace_name = if let Some(name) = &instance.workspace_name {
                Some(name.clone())
            } else if let Ok(Some((name, _, _, _))) =
                state_manager.get_sync_session(&instance.id).await
            {
                Some(name)
            } else {
                None
            };
            if let Some(workspace_name) = workspace_name {
                let probe = probe_remote_tilt(&workspace_name, REMOTE_STATUS_SSH_TIMEOUT_SECS);
                print_remote_tilt_probe(&probe);
                return Ok(());
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

fn print_remote_tilt_probe(probe: &RemoteTiltProbe) {
    let label = probe.label();
    let painted = match probe {
        RemoteTiltProbe::TiltRunning => label.green(),
        RemoteTiltProbe::TiltStopped => label.yellow(),
        RemoteTiltProbe::WorkspaceStopped => label.yellow(),
        RemoteTiltProbe::AgentUnavailable { .. } => label.yellow(),
        RemoteTiltProbe::TimedOut { .. } => label.yellow(),
    };
    println!("  {painted} (on remote workspace)");
    match probe {
        RemoteTiltProbe::TimedOut { timeout_secs } => {
            println!(
                "  SSH probe exceeded {timeout_secs}s; workspace may be stopped or the agent unavailable"
            );
        }
        RemoteTiltProbe::AgentUnavailable { detail } if !detail.is_empty() => {
            println!("  {detail}");
        }
        RemoteTiltProbe::WorkspaceStopped => {
            println!(
                "  Coder workspace is not running; start it before retrying status --detailed"
            );
        }
        _ => {}
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use devdock_monitor::{ResourceLabel, ResourceType};

    fn resource(name: &str, status: ResourceStatus) -> TiltResource {
        TiltResource {
            name: name.to_string(),
            labels: vec![ResourceLabel::Service],
            resource_type: ResourceType::Local,
            status,
            runtime_status: None,
            update_status: None,
            compose_health: None,
            is_disabled: false,
            build_start_time: None,
            build_finish_time: None,
            build_duration_secs: None,
            build_history: Vec::new(),
        }
    }

    #[test]
    fn format_resource_status_formats_all_statuses() {
        assert!(format_resource_status(&ResourceStatus::Healthy).contains("Healthy"));
        assert!(format_resource_status(&ResourceStatus::Pending).contains("Pending"));
        assert!(format_resource_status(&ResourceStatus::Failed).contains("Failed"));
        assert!(format_resource_status(&ResourceStatus::Disabled).contains("Disabled"));
        assert!(format_resource_status(&ResourceStatus::Unknown).contains("Unknown"));
    }

    #[test]
    fn count_resources_counts_statuses() {
        let resources = vec![
            resource("healthy", ResourceStatus::Healthy),
            resource("pending", ResourceStatus::Pending),
            resource("failed", ResourceStatus::Failed),
            resource("disabled", ResourceStatus::Disabled),
            resource("unknown", ResourceStatus::Unknown),
        ];

        assert_eq!(
            count_resources(&resources),
            ResourceCounts { healthy: 1, pending: 1, failed: 1, disabled: 1, total: 5 }
        );
    }

    #[test]
    fn resource_row_values_uses_dash_for_missing_optional_fields() {
        let values = resource_row_values(&resource("qe", ResourceStatus::Healthy));

        assert_eq!(values[0], "qe");
        assert_eq!(values[1], "Local");
        assert!(values[2].contains("Healthy"));
        assert_eq!(values[3], "-");
        assert_eq!(values[4], "-");
        assert_eq!(values[5], "-");
    }

    #[test]
    fn remote_tilt_probe_labels_match_required_states() {
        assert_eq!(RemoteTiltProbe::WorkspaceStopped.label(), "workspace stopped");
        assert_eq!(
            RemoteTiltProbe::AgentUnavailable { detail: "x".into() }.label(),
            "agent unavailable"
        );
        assert_eq!(RemoteTiltProbe::TimedOut { timeout_secs: 10 }.label(), "timed out");
        assert_eq!(RemoteTiltProbe::TiltRunning.label(), "Tilt is running");
        assert_eq!(RemoteTiltProbe::TiltStopped.label(), "Tilt is not running");
    }
}
