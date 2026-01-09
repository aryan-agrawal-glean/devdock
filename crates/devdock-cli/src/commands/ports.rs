//! Port management commands.

use anyhow::{anyhow, Result};
use colored::Colorize;
use devdock_state::StateManager;
use prettytable::{format, row, Table};
use std::fs::File;
use std::process::{Command, Stdio};

/// List port mappings (renamed from execute for clarity).
pub async fn list(instance: Option<String>) -> Result<()> {
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
        println!(
            "Tip: Use --auto-forward when starting remote DevDock to enable auto port forwarding"
        );
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
    table.add_row(row![
        "Service",
        "Type",
        "Local Port",
        "Remote Port",
        "Status"
    ]);

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
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| anyhow!("Failed to get git worktree: {}", e))?;

    if !output.status.success() {
        return Err(anyhow!("Not in a git repository"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Forward ports for remote DevDock.
pub async fn forward(all: bool, service: Option<&str>, custom: Option<&str>) -> Result<()> {
    let state = StateManager::new().await?;

    // Get current instance
    let (devdock_id, workspace_name, port_base) = get_current_remote_instance(&state).await?;

    if all {
        forward_all_ports(&state, &devdock_id, &workspace_name, port_base).await
    } else if let Some(svc) = service {
        forward_service_port(&state, &devdock_id, &workspace_name, port_base, svc).await
    } else if let Some(custom_mapping) = custom {
        forward_custom_port(&state, &devdock_id, &workspace_name, custom_mapping).await
    } else {
        Err(anyhow!(
            "Please specify --all, --service <name>, or --custom <local:remote>"
        ))
    }
}

/// Stop port forwarding.
pub async fn stop(all: bool, service: Option<&str>) -> Result<()> {
    let state = StateManager::new().await?;

    // Get current instance
    let (devdock_id, _, _) = get_current_remote_instance(&state).await?;

    if all {
        stop_all_port_forwards(&state, &devdock_id).await
    } else if let Some(svc) = service {
        stop_service_port_forward(&state, &devdock_id, svc).await
    } else {
        Err(anyhow!("Please specify --all or --service <name>"))
    }
}

/// Get current remote DevDock instance.
async fn get_current_remote_instance(state: &StateManager) -> Result<(String, String, u16)> {
    let worktree = get_current_worktree()?;
    let instances = state.list_instances().await?;

    let instance = instances
        .into_iter()
        .find(|i| {
            i.workspace_path == worktree && i.status.as_str() == "running" && i.mode == "remote"
        })
        .ok_or_else(|| anyhow!("No running remote DevDock instance found for current worktree"))?;

    let workspace_name = instance
        .workspace_name
        .clone()
        .ok_or_else(|| anyhow!("Remote instance missing workspace name"))?;

    Ok((instance.id, workspace_name, instance.port_base))
}

/// Common service port mappings.
fn get_service_ports() -> Vec<(&'static str, u16, u16)> {
    vec![
        ("tilt", 12350, 10350),     // Tilt UI
        ("qe", 7960, 7960),         // Query Engine
        ("fe", 8080, 8080),         // Frontend
        ("opensearch", 9200, 9200), // OpenSearch
        ("mysql", 3306, 3306),      // MySQL
        ("redis", 6379, 6379),      // Redis
    ]
}

/// Forward all common service ports.
async fn forward_all_ports(
    state: &StateManager,
    devdock_id: &str,
    workspace_name: &str,
    port_base: u16,
) -> Result<()> {
    println!(
        "{}",
        "Setting up port forwarding for all services...".cyan()
    );

    let services = get_service_ports();
    let mut success_count = 0;

    for (service_name, local_offset, remote_port) in &services {
        let local_port = port_base + *local_offset;

        match start_port_forward(
            state,
            devdock_id,
            workspace_name,
            service_name,
            local_port,
            *remote_port,
        )
        .await
        {
            Ok(_) => {
                println!(
                    "  ✅ {}: localhost:{} → remote:{}",
                    service_name.cyan(),
                    local_port,
                    remote_port
                );
                success_count += 1;
            }
            Err(e) => {
                println!("  ⚠️  {}: failed ({})", service_name.yellow(), e);
            }
        }
    }

    println!();
    println!(
        "{} {}/{} port forwards established",
        "✅".green(),
        success_count,
        services.len()
    );
    println!("View mappings: {}", "devdock ports list".cyan());

    Ok(())
}

/// Forward a specific service port.
async fn forward_service_port(
    state: &StateManager,
    devdock_id: &str,
    workspace_name: &str,
    port_base: u16,
    service_name: &str,
) -> Result<()> {
    let services = get_service_ports();

    let (_, local_offset, remote_port) = services
        .iter()
        .find(|(name, _, _)| *name == service_name)
        .ok_or_else(|| {
            anyhow!(
                "Unknown service: {}. Available: tilt, qe, fe, opensearch, mysql, redis",
                service_name
            )
        })?;

    let local_port = port_base + local_offset;

    println!("{}", format!("Forwarding {} port...", service_name).cyan());
    start_port_forward(
        state,
        devdock_id,
        workspace_name,
        service_name,
        local_port,
        *remote_port,
    )
    .await?;

    println!(
        "  ✅ {}: localhost:{} → remote:{}",
        service_name.cyan(),
        local_port,
        remote_port
    );
    println!();
    println!("View mappings: {}", "devdock ports list".cyan());

    Ok(())
}

/// Forward a custom port mapping.
async fn forward_custom_port(
    state: &StateManager,
    devdock_id: &str,
    workspace_name: &str,
    mapping: &str,
) -> Result<()> {
    let parts: Vec<&str> = mapping.split(':').collect();
    if parts.len() != 2 {
        return Err(anyhow!(
            "Invalid port mapping format. Use: local:remote (e.g., 8888:8888)"
        ));
    }

    let local_port: u16 = parts[0]
        .parse()
        .map_err(|_| anyhow!("Invalid local port: {}", parts[0]))?;
    let remote_port: u16 = parts[1]
        .parse()
        .map_err(|_| anyhow!("Invalid remote port: {}", parts[1]))?;

    println!("{}", "Forwarding custom port...".cyan());
    start_port_forward(
        state,
        devdock_id,
        workspace_name,
        "custom",
        local_port,
        remote_port,
    )
    .await?;

    println!(
        "  ✅ Custom: localhost:{} → remote:{}",
        local_port, remote_port
    );
    println!();
    println!("View mappings: {}", "devdock ports list".cyan());

    Ok(())
}

/// Start a port forward process.
async fn start_port_forward(
    state: &StateManager,
    devdock_id: &str,
    workspace_name: &str,
    service_name: &str,
    local_port: u16,
    remote_port: u16,
) -> Result<()> {
    // Create log directory
    let log_dir = dirs::home_dir()
        .ok_or_else(|| anyhow!("Could not determine home directory"))?
        .join(".devdock")
        .join("ports");
    std::fs::create_dir_all(&log_dir)?;

    let log_file = log_dir.join(format!("{}_{}.log", devdock_id, service_name));
    let log_handle = File::create(&log_file)?;

    // Start port forward process
    let child = Command::new("coder")
        .arg("port-forward")
        .arg(workspace_name)
        .arg("--tcp")
        .arg(format!("{}:{}", local_port, remote_port))
        .stdin(Stdio::null())
        .stdout(log_handle.try_clone()?)
        .stderr(log_handle)
        .spawn()
        .map_err(|e| anyhow!("Failed to start port forward: {}", e))?;

    let pid = child.id();

    // Give it a moment to establish
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    // Save port mapping to state
    state
        .save_port_mapping(
            devdock_id,
            service_name,
            "service",
            local_port,
            remote_port,
            Some(pid),
        )
        .await?;

    Ok(())
}

/// Stop all port forwards for an instance.
async fn stop_all_port_forwards(state: &StateManager, devdock_id: &str) -> Result<()> {
    let mappings = state.get_port_mappings(devdock_id).await?;

    if mappings.is_empty() {
        println!("{}", "No port forwards to stop".yellow());
        return Ok(());
    }

    println!("{}", "Stopping all port forwards...".cyan());
    let mut stopped_count = 0;

    for (service_name, _, _, _, pid_opt) in mappings {
        if let Some(pid) = pid_opt {
            if kill_process(pid) {
                println!("  ✅ Stopped: {}", service_name.cyan());
                stopped_count += 1;
            }
        }
        // Remove from state
        state.remove_port_mapping(devdock_id, &service_name).await?;
    }

    println!();
    println!("{} {} port forward(s) stopped", "✅".green(), stopped_count);

    Ok(())
}

/// Stop a specific service port forward.
async fn stop_service_port_forward(
    state: &StateManager,
    devdock_id: &str,
    service_name: &str,
) -> Result<()> {
    let mappings = state.get_port_mappings(devdock_id).await?;

    let mapping = mappings
        .iter()
        .find(|(name, _, _, _, _)| name == service_name)
        .ok_or_else(|| anyhow!("No port forward found for service: {}", service_name))?;

    println!(
        "{}",
        format!("Stopping {} port forward...", service_name).cyan()
    );

    if let Some(pid) = mapping.4 {
        if kill_process(pid) {
            println!("  ✅ Stopped: {} (PID: {})", service_name.cyan(), pid);
        } else {
            println!("  ⚠️  Process {} not found (may have already exited)", pid);
        }
    }

    // Remove from state
    state.remove_port_mapping(devdock_id, service_name).await?;

    println!();
    println!("View remaining mappings: {}", "devdock ports list".cyan());

    Ok(())
}

/// Kill a process by PID.
fn kill_process(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .arg(pid.to_string())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}
