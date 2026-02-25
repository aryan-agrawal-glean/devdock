//! Stop command implementation.

use crate::output;
use anyhow::{Context, Result};
use devdock_state::{Instance, InstanceStatus, StateManager};
use devdock_sync::daemon as sync_daemon;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub async fn execute(target: Option<&str>, force: bool, delete: bool) -> Result<()> {
    output::info("Stopping DevDock...");

    let state_manager = StateManager::new().await?;

    // Determine which instance to stop
    let instance_id = if let Some(target) = target {
        // Try to find instance by ID, branch, or workspace path
        resolve_instance_identifier(&state_manager, target).await?
    } else {
        // Use current workspace
        let workspace_path = std::env::current_dir()?.display().to_string();
        state_manager
            .get_instance_by_workspace(&workspace_path)
            .await?
            .map(|i| i.id)
            .context("No DevDock instance found for current workspace")?
    };

    // Get instance details
    let instance = state_manager.get_instance(&instance_id).await?.context("Instance not found")?;

    output::info(&format!(
        "Stopping instance: {} ({})",
        Instance::display_id(&instance_id),
        instance.branch
    ));

    // Update status to stopping
    state_manager.update_status(&instance_id, InstanceStatus::Stopping).await?;

    // Perform cleanup based on instance mode
    if instance.mode == "remote" {
        perform_remote_cleanup(&state_manager, &instance_id, force, delete).await?;
    } else {
        if force {
            output::info("Force stopping (skipping graceful shutdown)...");
        }
        perform_cleanup(&instance.workspace_path, force)?;
    }

    // Update status to stopped
    state_manager.update_status(&instance_id, InstanceStatus::Stopped).await?;

    output::success("DevDock stopped successfully");
    Ok(())
}

/// Resolve instance identifier (ID, branch, or workspace path) to instance ID
async fn resolve_instance_identifier(
    state_manager: &StateManager,
    identifier: &str,
) -> Result<String> {
    // Try as instance ID first (full or prefix)
    if let Ok(instances) = state_manager.list_instances().await {
        // Try as instance ID
        for instance in &instances {
            if instance.id == identifier || instance.id.starts_with(identifier) {
                return Ok(instance.id.clone());
            }
        }

        // Try as branch name
        for instance in &instances {
            if instance.branch == identifier {
                return Ok(instance.id.clone());
            }
        }

        // Try as workspace path
        for instance in &instances {
            if instance.workspace_path.ends_with(identifier) {
                return Ok(instance.id.clone());
            }
        }
    }

    anyhow::bail!("No instance found matching: {identifier}")
}

/// Perform cleanup operations
fn perform_cleanup(workspace_path: &str, force: bool) -> Result<()> {
    output::info("Cleaning up DevDock resources...");

    // 1. Stop Tilt if running
    stop_tilt(workspace_path, force)?;

    // 2. Stop Docker containers
    cleanup_docker_containers()?;

    // 3. Kill processes on ports
    cleanup_port_processes()?;

    // 4. Clean up dangling images (only if force)
    if force {
        cleanup_dangling_images()?;
    }

    // 5. Clean up temporary files
    cleanup_temp_files(workspace_path)?;

    output::success("Cleanup complete");
    Ok(())
}

/// Stop Tilt process
fn stop_tilt(_workspace_path: &str, force: bool) -> Result<()> {
    output::info("Stopping Tilt...");

    #[cfg(unix)]
    {
        // Try to find tilt process
        let output = Command::new("pgrep").args(["-f", "tilt up"]).output();

        if let Ok(output) = output {
            if output.status.success() {
                let pids = String::from_utf8_lossy(&output.stdout);
                for pid_str in pids.lines() {
                    if let Ok(pid) = pid_str.parse::<i32>() {
                        unsafe {
                            if force {
                                // Force kill
                                libc::kill(pid, libc::SIGKILL);
                            } else {
                                // Graceful shutdown
                                libc::kill(pid, libc::SIGTERM);
                                // Wait a bit
                                std::thread::sleep(std::time::Duration::from_secs(2));
                                // Force if still running
                                libc::kill(pid, libc::SIGKILL);
                            }
                        }
                        output::success(&format!("Stopped Tilt process: {pid}"));
                    }
                }
            }
        }
    }

    Ok(())
}

/// Stop all DevDock Docker containers
fn cleanup_docker_containers() -> Result<()> {
    output::info("Stopping Docker containers...");

    // Get all devdock containers
    let output = Command::new("docker")
        .args(["ps", "-a", "--filter", "name=devdock-", "--format", "{{.Names}}"])
        .output()?;

    if output.status.success() {
        let containers = String::from_utf8_lossy(&output.stdout);
        let container_list: Vec<&str> = containers.lines().filter(|s| !s.is_empty()).collect();

        if !container_list.is_empty() {
            // Stop containers
            for container in &container_list {
                Command::new("docker")
                    .args(["stop", container])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .ok();
            }

            // Remove containers
            for container in &container_list {
                Command::new("docker")
                    .args(["rm", "-f", container])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .ok();
            }

            output::success(&format!("Stopped {} container(s)", container_list.len()));
        }
    }

    Ok(())
}

/// Kill processes on specific ports
fn cleanup_port_processes() -> Result<()> {
    output::info("Cleaning up processes on ports...");

    let ports = vec![8888, 8880, 7070, 8090, 7972, 3407, 3408, 10350]; // Added Tilt port
    let mut killed = 0;

    for port in ports {
        #[cfg(unix)]
        {
            // Use lsof to find processes on port
            let output = Command::new("lsof").args(["-i", &format!(":{port}"), "-t"]).output();

            if let Ok(output) = output {
                if output.status.success() {
                    let pids = String::from_utf8_lossy(&output.stdout);
                    for pid_str in pids.lines() {
                        if let Ok(pid) = pid_str.parse::<i32>() {
                            unsafe {
                                // Try SIGTERM first
                                if libc::kill(pid, libc::SIGTERM) == 0 {
                                    killed += 1;
                                    // Wait a bit
                                    std::thread::sleep(std::time::Duration::from_secs(1));
                                    // Force kill if still running
                                    libc::kill(pid, libc::SIGKILL);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if killed > 0 {
        output::success(&format!("Stopped {killed} process(es) on ports"));
    }

    Ok(())
}

/// Remove dangling Docker images
fn cleanup_dangling_images() -> Result<()> {
    output::info("Cleaning up dangling images...");

    let output =
        Command::new("docker").args(["images", "--filter", "dangling=true", "-q"]).output()?;

    if output.status.success() {
        let images = String::from_utf8_lossy(&output.stdout);
        let image_list: Vec<&str> = images.lines().filter(|s| !s.is_empty()).collect();

        if !image_list.is_empty() {
            for image in &image_list {
                Command::new("docker")
                    .args(["rmi", image])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .ok();
            }
            output::success(&format!("Removed {} dangling image(s)", image_list.len()));
        }
    }

    Ok(())
}

/// Clean up temporary files
fn cleanup_temp_files(workspace_path: &str) -> Result<()> {
    output::info("Cleaning up temporary files...");

    let home = std::env::var("HOME")?;
    let paths_to_clean = vec![
        format!("{}/.devdock/opensearch/full_cleanup.txt", home),
        format!("{}/.devdock/sql/full_cleanup.txt", home),
        format!("{}/.devdock/resource_monitor.yaml", home),
        format!("{}/.devdock/resource_monitor.log", home),
        format!("{}/.devdock/startup-monitor.pid", home),
        format!("{}/.devdock/session_timing.txt", home),
    ];

    let mut cleaned_count = 0;
    for path in paths_to_clean {
        if std::fs::remove_file(&path).is_ok() {
            cleaned_count += 1;
        }
    }

    // Clean up runtime_generated directory
    let workspace = PathBuf::from(workspace_path);
    output::info(&format!("Workspace path: {}", workspace.display()));

    let runtime_gen = workspace.join("devdock/runtime_generated");
    output::info(&format!("Looking for runtime_generated at: {}", runtime_gen.display()));

    if runtime_gen.exists() {
        output::info("Removing runtime_generated directory...");
        match std::fs::remove_dir_all(&runtime_gen) {
            Ok(()) => {
                output::success("Removed runtime_generated directory");
                cleaned_count += 1;
            }
            Err(e) => {
                output::warn(&format!("Failed to remove runtime_generated: {e}"));
            }
        }
    } else {
        output::info("runtime_generated directory not found (may already be clean)");
    }

    // Clean up devdock.ini files
    let config_files = vec![
        workspace.join("devdock/devdock.ini"),
        workspace.join("config/devdock.ini"),
        workspace.join("config/run_local.ini"),
        workspace.join("devdock/scio_apps.ini"),
    ];

    for config_file in config_files {
        if config_file.exists() && std::fs::remove_file(&config_file).is_ok() {
            cleaned_count += 1;
        }
    }

    output::success(&format!("Cleaned {cleaned_count} file(s)/directory(s)"));
    Ok(())
}

/// Perform cleanup for remote DevDock instances
async fn perform_remote_cleanup(
    state_manager: &StateManager,
    instance_id: &str,
    force: bool,
    delete: bool,
) -> Result<()> {
    output::info("Cleaning up remote DevDock resources...");

    // 0. Clean up local state files
    let home = std::env::var("HOME").unwrap_or_default();
    let state_files = vec![
        format!("{}/.devdock/startup-monitor.pid", home),
        format!("{}/.devdock/session_timing.txt", home),
        format!("{}/.devdock/resource_monitor.yaml", home),
        format!("{}/.devdock/resource_monitor.log", home),
    ];

    for file_path in state_files {
        let _ = std::fs::remove_file(&file_path);
    }

    // Get workspace name BEFORE cleaning up (we'll need it for prompts later)
    let workspace_name =
        if let Ok(Some((ws_name, _, _, _))) = state_manager.get_sync_session(instance_id).await {
            println!("DEBUG: Found workspace name from sync session: {ws_name}");
            Some(ws_name)
        } else {
            println!("DEBUG: No sync session found for instance {instance_id}");
            None
        };

    // 1. Stop file sync
    output::info("Stopping file sync...");
    if let Err(e) = sync_daemon::stop_daemon(instance_id) {
        output::warn(&format!("Failed to stop sync daemon: {e}"));
    } else {
        output::success("File sync stopped");
    }

    // Clean up sync session from database
    if let Err(e) = state_manager.delete_sync_session(instance_id).await {
        output::warn(&format!("Failed to clean sync session: {e}"));
    }

    // 2. Stop port forwards
    output::info("Stopping port forwards...");

    let mut stopped_count = 0;

    // First try to kill PIDs from database
    let mappings = state_manager.get_port_mappings(instance_id).await?;
    for (_service_name, _port_type, _local_port, _remote_port, pid) in &mappings {
        if let Some(pid) = pid {
            #[cfg(unix)]
            {
                unsafe {
                    if libc::kill(*pid as i32, libc::SIGTERM) == 0 {
                        stopped_count += 1;
                    }
                }
            }

            #[cfg(windows)]
            {
                let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).output();
                stopped_count += 1;
            }
        }
    }

    // Also search for and kill any port-forward processes for this workspace
    if let Some(ref ws_name) = workspace_name {
        #[cfg(unix)]
        {
            let output = Command::new("pgrep")
                .args(["-f", &format!("coder port-forward {ws_name}")])
                .output();

            if let Ok(output) = output {
                if output.status.success() {
                    let pids = String::from_utf8_lossy(&output.stdout);
                    for pid_str in pids.lines() {
                        if let Ok(pid) = pid_str.parse::<i32>() {
                            unsafe {
                                if libc::kill(pid, libc::SIGTERM) == 0 {
                                    stopped_count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if stopped_count > 0 {
        output::success(&format!("Stopped {stopped_count} port forward(s)"));
    }

    // Clean up port mappings from database
    if let Err(e) = state_manager.delete_port_mappings(instance_id).await {
        output::warn(&format!("Failed to clean port mappings: {e}"));
    }

    // 3. Prompt for workspace cleanup (skip prompt if force or delete flag)
    println!("DEBUG: workspace_name = {workspace_name:?}, force = {force}, delete = {delete}");
    if let Some(ref workspace_name) = workspace_name {
        if delete {
            // With --delete, non-interactively delete the workspace
            output::info(&format!("Deleting workspace: {workspace_name}"));

            match Command::new("coder").arg("delete").arg(workspace_name).arg("--yes").output() {
                Ok(output) => {
                    if output.status.success() {
                        output::success(&format!("Workspace '{workspace_name}' deleted"));
                    } else {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        output::warn(&format!("Failed to delete workspace: {stderr}"));
                    }
                }
                Err(e) => {
                    output::warn(&format!("Failed to delete workspace: {e}"));
                }
            }
        } else if force {
            // With --force, just leave workspace running
            output::info(&format!(
                "Workspace '{workspace_name}' left running (use --force to skip prompts)"
            ));
            output::info(&format!("  Stop: coder stop {workspace_name}"));
            output::info(&format!("  Delete: coder delete {workspace_name}"));
        } else {
            // Interactive prompt
            println!();
            println!("  What would you like to do with workspace '{workspace_name}'?");
            println!("  1) Leave running (default)");
            println!("  2) Stop workspace");
            println!("  3) Delete workspace");
            println!();
            print!("  Enter choice (1-3): ");
            use std::io::{self, Write};
            io::stdout().flush()?;

            let mut choice = String::new();
            io::stdin().read_line(&mut choice)?;
            let choice = choice.trim();

            match choice {
                "2" => {
                    output::info(&format!("Stopping workspace: {workspace_name}"));

                    match Command::new("coder")
                        .arg("stop")
                        .arg(workspace_name)
                        .arg("--yes")
                        .output()
                    {
                        Ok(output) => {
                            if output.status.success() {
                                output::success(&format!("Workspace '{workspace_name}' stopped"));
                            } else {
                                let stderr = String::from_utf8_lossy(&output.stderr);
                                output::warn(&format!("Failed to stop workspace: {stderr}"));
                            }
                        }
                        Err(e) => {
                            output::warn(&format!("Failed to stop workspace: {e}"));
                        }
                    }
                }
                "3" => {
                    output::info(&format!("Deleting workspace: {workspace_name}"));

                    match Command::new("coder")
                        .arg("delete")
                        .arg(workspace_name)
                        .arg("--yes")
                        .output()
                    {
                        Ok(output) => {
                            if output.status.success() {
                                output::success(&format!("Workspace '{workspace_name}' deleted"));
                            } else {
                                let stderr = String::from_utf8_lossy(&output.stderr);
                                output::warn(&format!("Failed to delete workspace: {stderr}"));
                            }
                        }
                        Err(e) => {
                            output::warn(&format!("Failed to delete workspace: {e}"));
                        }
                    }
                }
                "1" | "" => {
                    output::info(&format!("Workspace '{workspace_name}' left running"));
                    output::info(&format!("  Stop: coder stop {workspace_name}"));
                    output::info(&format!("  Delete: coder delete {workspace_name}"));
                }
                _ => {
                    output::info("Invalid choice. Workspace left running.");
                }
            }
        }
    }

    output::success("Remote cleanup complete");
    Ok(())
}
