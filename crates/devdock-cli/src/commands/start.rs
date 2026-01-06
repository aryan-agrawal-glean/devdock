//! Start command implementation.

use crate::output;
use anyhow::{Context, Result};
use devdock_config::{DependencyResolver, ProfileLoader, ProfileValidator};
use devdock_state::{Instance, InstanceStatus, PortAllocator, StateManager};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub async fn execute(profile_name: &str, services: Option<&str>, _ci_mode: bool) -> Result<()> {
    output::info("Starting DevDock...");

    // Step 1: Load and validate profile
    output::info(&format!("Loading profile: {}", profile_name));
    let loader = ProfileLoader::new();
    let (profile, profile_path) = loader.load_with_path(profile_name)?;

    let validator = ProfileValidator::new();
    validator.validate(&profile)?;
    validator.validate_gcloud_project(&profile)?;
    output::success("Profile validated");

    // Step 2: Resolve dependencies
    let resolver = DependencyResolver::new();
    let resolved_components = resolver.resolve(&profile)?;
    output::info(&format!(
        "Resolved {} components",
        resolved_components.len()
    ));

    // Step 3: Check Docker is running
    check_docker()?;

    // Step 4: Setup Docker networks
    setup_docker_networks()?;

    // Step 5: Register instance and allocate ports
    let state_manager = StateManager::new().await?;
    let mut port_allocator = PortAllocator::new();
    let port_base = port_allocator.allocate()?;

    let workspace_path = std::env::current_dir()?.display().to_string();
    let branch = get_git_branch()?;

    let instance = Instance {
        id: Instance::generate_id(),
        workspace_path: workspace_path.clone(),
        branch,
        profile: profile_name.to_string(),
        mode: "local".to_string(),
        status: InstanceStatus::Starting,
        port_base,
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs() as i64,
        updated_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs() as i64,
    };

    state_manager.register_instance(&instance).await?;
    output::success(&format!(
        "Registered instance: {} (port base: {})",
        &instance.id[..8],
        port_base
    ));

    // Step 6: Generate configuration files (Rust - parallel)
    let workspace_root = find_workspace_root()?;
    let profile_path_str = profile_path.to_string_lossy();
    generate_configs_rust(&workspace_root, &profile, services, Some(&profile_path_str)).await?;

    // Step 7: Clean old logs and startup state files
    cleanup_old_logs()?;
    cleanup_startup_state_files()?;

    // Step 8: Update status and start Tilt
    state_manager
        .update_status(&instance.id, InstanceStatus::Running)
        .await?;

    output::info("Starting Tilt...");
    let tilt_process = start_tilt(services)?;

    output::success("DevDock started!");
    println!("\n  Tilt UI: http://localhost:10350");
    println!("  Services: https://local.glean.com:{}", port_base);
    println!("\nPress Ctrl+C to stop DevDock\n");

    // Wait for Tilt process
    wait_for_tilt(tilt_process, &state_manager, &instance.id, workspace_root).await?;

    Ok(())
}

/// Check if Docker is running
fn check_docker() -> Result<()> {
    output::info("Checking Docker...");

    let output = Command::new("docker")
        .arg("info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("Failed to run 'docker info'")?;

    if !output.success() {
        anyhow::bail!(
            "Docker is not running. Please start Docker and try again.\n\
             To start Docker (macOS with colima):\n\
             colima start --memory 10 --cpu 4 --network-address slirp --vm-type vz"
        );
    }

    output::success("Docker is running");
    Ok(())
}

/// Setup Docker networks
fn setup_docker_networks() -> Result<()> {
    output::info("Setting up Docker networks...");

    for network in &["devdock_network", "devdock_infra"] {
        Command::new("docker")
            .args(["network", "create", network])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok(); // Ignore errors if network already exists
    }

    output::success("Docker networks ready");
    Ok(())
}

/// Get current git branch
fn get_git_branch() -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .context("Failed to get git branch")?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Ok("no-branch".to_string())
    }
}

/// Generate configurations using Rust (parallel)
async fn generate_configs_rust(
    workspace_root: &Path,
    profile: &devdock_core::profile::Profile,
    services: Option<&str>,
    profile_path: Option<&str>,
) -> Result<()> {
    use devdock_config::{ConfigGenerator, WatchStrategy};

    // Use Hybrid strategy: instant static patterns + background cquery refinement
    let generator = ConfigGenerator::new(workspace_root.to_path_buf())
        .with_watch_strategy(WatchStrategy::Hybrid);
    generator.generate(profile, services, profile_path).await?;

    Ok(())
}


/// Clean up old logs
fn cleanup_old_logs() -> Result<()> {
    output::info("Cleaning up old logs...");

    let home = std::env::var("HOME")?;
    let devdock_dir = PathBuf::from(&home).join(".devdock");

    if devdock_dir.exists() {
        // Clean log files from service directories
        for entry in std::fs::read_dir(&devdock_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let dir_name = path.file_name().unwrap().to_string_lossy();

                // Skip special directories
                if matches!(dir_name.as_ref(), "images" | "auth" | "sql" | "grafana" | "bazel" | "generated" | "sync") {
                    continue;
                }

                // Remove log files
                if let Ok(entries) = std::fs::read_dir(&path) {
                    for log_entry in entries.flatten() {
                        let log_path = log_entry.path();
                        if log_path.extension().and_then(|s| s.to_str()) == Some("log") {
                            std::fs::remove_file(log_path).ok();
                        }
                    }
                }
            }
        }
    }

    output::success("Old logs cleaned");
    Ok(())
}

/// Clean up startup state files to prevent stale state from blocking new startup
fn cleanup_startup_state_files() -> Result<()> {
    output::info("Cleaning up startup state files...");

    let home = std::env::var("HOME")?;
    let state_files = vec![
        format!("{}/.devdock/startup-monitor.pid", home),
        format!("{}/.devdock/session_timing.txt", home),
    ];

    let mut cleaned_count = 0;
    for path in state_files {
        if std::fs::remove_file(&path).is_ok() {
            cleaned_count += 1;
        }
    }

    if cleaned_count > 0 {
        output::success(&format!("Cleaned {} startup state file(s)", cleaned_count));
    }
    Ok(())
}

/// Start Tilt process
fn start_tilt(services: Option<&str>) -> Result<Child> {
    // Find monorepo root (where devdock directory is)
    let workspace_root = find_workspace_root()?;
    let devdock_dir = workspace_root.join("devdock");

    if !devdock_dir.exists() {
        anyhow::bail!(
            "devdock directory not found at {}. Are you in the monorepo?",
            workspace_root.display()
        );
    }

    // Set DEVDOCK_ROOT environment variable
    std::env::set_var("DEVDOCK_ROOT", &workspace_root);

    // Build tilt command
    let mut cmd = Command::new("tilt");
    cmd.arg("up");
    cmd.current_dir(&devdock_dir);

    // Add services if specified
    if let Some(svcs) = services {
        for service in svcs.split(',') {
            cmd.arg(service.trim());
        }
        // Add helper services
        cmd.args(["gcloud-auth", "trigger-profile-update", "current-config", "startup-monitor"]);
    }

    // Spawn tilt process
    let child = cmd
        .stdin(Stdio::null())
        .spawn()
        .context("Failed to start tilt")?;

    Ok(child)
}

/// Find workspace root (directory containing devdock/)
fn find_workspace_root() -> Result<PathBuf> {
    find_workspace_root_from(&std::env::current_dir()?)
}


/// Find workspace root from a specific starting path
fn find_workspace_root_from(start: &Path) -> Result<PathBuf> {
    let mut current = start.to_path_buf();

    loop {
        if current.join("devdock").exists() && current.join("devdock").is_dir() {
            return Ok(current);
        }

        if !current.pop() {
            anyhow::bail!(
                "Could not find workspace root (directory with devdock/).\n\
                 Are you running this from the Scio monorepo?"
            );
        }
    }
}

/// Wait for Tilt process and handle shutdown
async fn wait_for_tilt(
    mut tilt_process: Child,
    state_manager: &StateManager,
    instance_id: &str,
    workspace_root: PathBuf,
) -> Result<()> {
    let instance_id = instance_id.to_string();
    let workspace_root = workspace_root.clone();
    
    // Get PID before moving the process
    let tilt_pid = tilt_process.id();

    // Convert std Child to tokio Command for async waiting
    let mut tilt_handle = tokio::task::spawn_blocking(move || tilt_process.wait());

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            output::info("Received shutdown signal, stopping DevDock...");

            // Update status to stopping
            state_manager
                .update_status(&instance_id, InstanceStatus::Stopping)
                .await
                .ok();

            // Kill Tilt process
            #[cfg(unix)]
            {
                output::info(&format!("Terminating Tilt process: {}", tilt_pid));
                // Send SIGTERM
                unsafe {
                    libc::kill(tilt_pid as i32, libc::SIGTERM);
                }
                // Wait a bit for graceful shutdown
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

                // Check if still running, send SIGKILL
                if unsafe { libc::kill(tilt_pid as i32, 0) } == 0 {
                    unsafe {
                        libc::kill(tilt_pid as i32, libc::SIGKILL);
                    }
                }
            }

            // Perform cleanup
            perform_cleanup(&instance_id, &workspace_root).await?;

            // Update status
            state_manager
                .update_status(&instance_id, InstanceStatus::Stopped)
            .await
            .ok();

            output::success("DevDock stopped successfully");
        }
        result = &mut tilt_handle => {
            // Tilt exited on its own
            match result {
                Ok(Ok(status)) => {
                    // Perform cleanup
                    perform_cleanup(&instance_id, &workspace_root).await?;

    // Update status
    state_manager
                        .update_status(&instance_id, InstanceStatus::Stopped)
        .await?;

    if status.success() {
        output::success("DevDock stopped");
    } else {
        output::warn(&format!("Tilt exited with status: {}", status));
                    }
                }
                Ok(Err(e)) => {
                    output::warn(&format!("Tilt process error: {}", e));
                    perform_cleanup(&instance_id, &workspace_root).await?;
                    state_manager
                        .update_status(&instance_id, InstanceStatus::Stopped)
                        .await?;
                }
                Err(e) => {
                    output::warn(&format!("Failed to wait for Tilt: {}", e));
                    perform_cleanup(&instance_id, &workspace_root).await?;
                    state_manager
                        .update_status(&instance_id, InstanceStatus::Stopped)
                        .await?;
                }
            }
        }
    }

    Ok(())
}

/// Perform cleanup operations
async fn perform_cleanup(_instance_id: &str, workspace_root: &Path) -> Result<()> {
    output::info("Cleaning up DevDock resources...");

    // 1. Stop Docker containers
    cleanup_docker_containers()?;

    // 2. Kill processes on ports
    cleanup_port_processes()?;

    // 3. Clean up dangling images
    cleanup_dangling_images()?;

    // 4. Clean up temporary files
    cleanup_temp_files(workspace_root)?;

    output::success("Cleanup complete");
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
fn cleanup_temp_files(workspace_root: &Path) -> Result<()> {
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
    output::info(&format!("Workspace root: {}", workspace_root.display()));

    let runtime_gen = workspace_root.join("devdock/runtime_generated");
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

    // Also clean up config files
    let config_files = vec![
        workspace_root.join("devdock/devdock.ini"),
        workspace_root.join("config/devdock.ini"),
        workspace_root.join("config/run_local.ini"),
        workspace_root.join("devdock/scio_apps.ini"),
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
