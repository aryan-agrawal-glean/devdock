//! Stop command implementation.

use crate::output;
use anyhow::{Context, Result};
use devdock_state::{InstanceStatus, StateManager};
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub async fn execute(target: Option<&str>, force: bool) -> Result<()> {
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
    let instance = state_manager
        .get_instance(&instance_id)
        .await?
        .context("Instance not found")?;

    output::info(&format!(
        "Stopping instance: {} ({})",
        &instance_id[..8],
        instance.branch
    ));

    // Update status to stopping
    state_manager
        .update_status(&instance_id, InstanceStatus::Stopping)
        .await?;

    // Perform cleanup
    if force {
        output::info("Force stopping (skipping graceful shutdown)...");
    }

    perform_cleanup(&instance.workspace_path, force)?;

    // Update status to stopped
    state_manager
        .update_status(&instance_id, InstanceStatus::Stopped)
        .await?;

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

    anyhow::bail!("No instance found matching: {}", identifier)
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
        let output = Command::new("pgrep")
            .args(["-f", "tilt up"])
            .output();

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
                        output::success(&format!("Stopped Tilt process: {}", pid));
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
        .args([
            "ps",
            "-a",
            "--filter",
            "name=devdock-",
            "--format",
            "{{.Names}}",
        ])
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
            let output = Command::new("lsof")
                .args(["-i", &format!(":{}", port), "-t"])
                .output();

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
        output::success(&format!("Stopped {} process(es) on ports", killed));
    }

    Ok(())
}

/// Remove dangling Docker images
fn cleanup_dangling_images() -> Result<()> {
    output::info("Cleaning up dangling images...");

    let output = Command::new("docker")
        .args(["images", "--filter", "dangling=true", "-q"])
        .output()?;

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
            Ok(_) => {
                output::success("Removed runtime_generated directory");
                cleaned_count += 1;
            }
            Err(e) => {
                output::warn(&format!("Failed to remove runtime_generated: {}", e));
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
        if config_file.exists()
            && std::fs::remove_file(&config_file).is_ok() {
            cleaned_count += 1;
        }
    }

    output::success(&format!("Cleaned {} file(s)/directory(s)", cleaned_count));
    Ok(())
}

