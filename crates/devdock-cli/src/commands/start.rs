//! Start command implementation.

use crate::output;
use anyhow::{Context, Result};
use colored::Colorize;
use devdock_config::{DependencyResolver, ProfileLoader, ProfileValidator};
use devdock_state::{Instance, InstanceStatus, PortAllocator, SessionOrigin, StateManager};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    profile_name: &str,
    services: Option<&str>,
    ci_mode: bool,
    run_monitor: bool,
    monitor_timeout: Option<u64>,
    telemetry_enabled: bool,
    dry_run: bool,
    enable_logs: bool,
) -> Result<()> {
    // Track command start time for telemetry
    let command_start_time = chrono::Utc::now();

    output::info("Starting DevDock...");

    // Step 0: Enable rich logging if requested (skip in dry-run mode)
    if enable_logs && !dry_run {
        output::info("Enabling rich logging...");
        if let Err(e) = crate::commands::logs::enable_quiet().await {
            output::warn(&format!("Failed to enable rich logging: {e}"));
        } else {
            output::success("Rich logging enabled → http://localhost:3000/explore");
        }
    }

    // Step 1: Load and validate profile
    output::info(&format!("Loading profile: {profile_name}"));
    let loader = ProfileLoader::new();
    let (profile, profile_path) = loader.load_with_path(profile_name)?;

    let validator = ProfileValidator::new();
    validator.validate(&profile)?;

    // Skip gcloud project validation in dry-run mode (for benchmarking/CI)
    if !dry_run {
        validator.validate_gcloud_project(&profile)?;
    }
    output::success("Profile validated");

    // Set CLOUD_PLATFORM environment variable based on profile platform
    // This is needed for platform-specific config loading (e.g., devdock_config_aws.json)
    // Python code expects uppercase values: "AWS", "GOOGLE", "AZURE"
    let platform_str = profile.platform.to_string().to_uppercase();
    std::env::set_var("CLOUD_PLATFORM", &platform_str);

    // Step 2: Resolve dependencies
    let resolver = DependencyResolver::new();
    let resolved_components = resolver.resolve(&profile)?;
    output::info(&format!("Resolved {} components", resolved_components.len()));

    // Step 3: Check Docker is running (skip in dry-run mode)
    if !dry_run {
        check_docker()?;
    }

    // Step 4: Setup Docker networks (skip in dry-run mode)
    if !dry_run {
        setup_docker_networks()?;
    }

    // Determine session origin from ci_mode flag
    let origin = if ci_mode { SessionOrigin::Bot } else { SessionOrigin::Dev };

    // Step 5: Register instance and allocate ports (skip in dry-run mode)
    let (state_manager, instance_id, _port_base) = if !dry_run {
        let state_manager = StateManager::new().await?;
        let port_allocator = PortAllocator::new(state_manager.clone()).await;
        let port_base = port_allocator.allocate().await?;

        let workspace_path = std::env::current_dir()?.display().to_string();
        let branch = get_git_branch()?;

        // Check if DEVDOCK_ID is already set (from remote mode)
        // If set, use it for correlation; otherwise generate new ID
        let instance_id = std::env::var("DEVDOCK_ID")
            .ok()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| Instance::generate_id(origin));

        let instance = Instance {
            id: instance_id.clone(),
            workspace_path: workspace_path.clone(),
            workspace_name: None,
            branch,
            profile: profile_name.to_string(),
            mode: "local".to_string(),
            status: InstanceStatus::Starting,
            port_base,
            created_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64,
            updated_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64,
        };

        state_manager.register_instance(&instance).await?;
        output::success(&format!(
            "Registered instance: {} (port base: {})",
            Instance::display_id(&instance.id),
            port_base
        ));

        (Some(state_manager), instance_id, port_base)
    } else {
        // Generate a real ID even in dry-run for display safety, but skip DB registration
        (None, Instance::generate_id(origin), 0)
    };

    // Set DEVDOCK_ID early so config generation can inject it into service env vars
    if !instance_id.is_empty() {
        std::env::set_var("DEVDOCK_ID", &instance_id);
    }

    // Step 6: Generate configuration files (Rust - parallel)
    let workspace_root = find_workspace_root()?;
    let profile_path_str = profile_path.to_string_lossy();
    generate_configs_rust(&workspace_root, &profile, services, Some(&profile_path_str), dry_run)
        .await?;

    // Step 7: Clean old logs and startup state files
    cleanup_old_logs()?;
    cleanup_startup_state_files()?;

    // If dry-run mode, exit here with timing information
    if dry_run {
        let elapsed = chrono::Utc::now() - command_start_time;
        let elapsed_ms = elapsed.num_milliseconds();

        // Output timing in JSON format for benchmarking
        println!("{{\"profile\": \"{}\", \"bootstrap_time_ms\": {}}}", profile_name, elapsed_ms);

        output::success(&format!("Dry-run completed in {}ms", elapsed_ms));

        // Force exit to avoid tokio runtime shutdown hang (benchmarking only)
        std::process::exit(0);
    }

    // Unwrap state_manager (safe because dry_run is false)
    let state_manager = state_manager.unwrap();

    // Step 8: Update status and start Tilt
    state_manager.update_status(&instance_id, InstanceStatus::Running).await?;

    // Write profile name to state file for telemetry
    write_profile_state(profile_name)?;

    output::info("Starting Tilt...");
    let tilt_process = start_tilt(services, profile_name)?;

    // Track tilt up time for telemetry
    let tilt_up_time = chrono::Utc::now();

    let start_time = (chrono::Utc::now() - chrono::Duration::minutes(5))
        .format("%Y-%m-%dT%H:%M:%SZ");
    let gcp_logs_url = format!(
        "https://console.cloud.google.com/logs/query;query=labels.devdock_id%3D%22{instance_id}%22;summaryFields=labels%252Flogger_source:false:32:beginning;startTime={start_time};duration=PT6H?project={}",
        profile.project
    );

    output::success("DevDock started!");
    println!();
    println!("  {}", "──────────────────────────────────────────────────────────".cyan());
    println!("  {} http://localhost:10350", "Tilt UI:".cyan().bold());
    println!("  {} {}", "Session:".cyan().bold(), instance_id.dimmed());
    println!("  {} {}", "Logs:   ".cyan().bold(), gcp_logs_url.underline());
    println!("  {}", "──────────────────────────────────────────────────────────".cyan());

    if run_monitor {
        println!("\nResource monitor enabled (timeout: {}s)...\n", monitor_timeout.unwrap_or(1200));
    } else {
        println!("\n  {}", "Press Ctrl+C to stop DevDock".dimmed());
    }

    // Wait for Tilt process (optionally with monitoring)
    wait_for_tilt(
        tilt_process,
        &state_manager,
        &instance_id,
        workspace_root,
        run_monitor,
        monitor_timeout,
        telemetry_enabled,
        profile_name,
        command_start_time,
        tilt_up_time,
    )
    .await?;

    Ok(())
}

/// Check if Docker is running
fn check_docker() -> Result<()> {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    output::info("Checking Docker...");

    // Run docker info with a timeout to prevent hanging
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let result =
            Command::new("docker").arg("info").stdout(Stdio::null()).stderr(Stdio::null()).status();
        tx.send(result).ok();
    });

    // Wait for result with 10 second timeout
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(status)) if status.success() => {
            output::success("Docker is running");
            Ok(())
        }
        Ok(Ok(_)) => {
            anyhow::bail!(
                "Docker is not running. Please start Docker and try again.\n\
                 \n\
                 To start Docker:\n\
                 • macOS with colima: colima start --memory 10 --cpu 4 --network-address slirp --vm-type vz\n\
                 • Linux: sudo systemctl start docker"
            )
        }
        Ok(Err(e)) => {
            anyhow::bail!("Failed to run 'docker info': {e}")
        }
        Err(_timeout) => {
            anyhow::bail!(
                "Docker check timed out after 10 seconds.\n\
                 \n\
                 This usually means:\n\
                 1. Docker daemon is not running\n\
                 2. Docker daemon is stuck/unresponsive\n\
                 \n\
                 Try:\n\
                 • Restart Docker: colima start --memory 10 --cpu 4 --network-address slirp --vm-type vz (macOS) or sudo systemctl restart docker (Linux)\n\
                 • Check Docker status: docker ps\n\
                 • Kill stuck processes: pkill -9 docker"
            )
        }
    }
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
    dry_run: bool,
) -> Result<()> {
    use devdock_config::{ConfigGenerator, WatchStrategy};

    let mut generator = ConfigGenerator::new(workspace_root.to_path_buf())
        .with_watch_strategy(WatchStrategy::Static);

    // Skip platform initialization and file writes in dry-run mode (for benchmarking)
    if dry_run {
        generator = generator
            .with_skip_platform_init(true)
            .with_skip_file_writes(true)
            .with_dynamic_config(false);
    }

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
                if matches!(
                    dir_name.as_ref(),
                    "images" | "auth" | "sql" | "grafana" | "bazel" | "generated" | "sync"
                ) {
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
        output::success(&format!("Cleaned {cleaned_count} startup state file(s)"));
    }
    Ok(())
}

/// Start Tilt process
fn start_tilt(services: Option<&str>, profile_name: &str) -> Result<Child> {
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

    // Set DEVDOCK_RUST_BINARY to indicate the Rust implementation is being used
    std::env::set_var("DEVDOCK_RUST_BINARY", "true");

    // Set DEVDOCK_PROFILE for telemetry in monitor command
    std::env::set_var("DEVDOCK_PROFILE", profile_name);

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
    let child = cmd.stdin(Stdio::null()).spawn().context("Failed to start tilt")?;

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
#[allow(clippy::too_many_arguments)]
async fn wait_for_tilt(
    mut tilt_process: Child,
    state_manager: &StateManager,
    instance_id: &str,
    workspace_root: PathBuf,
    run_monitor: bool,
    monitor_timeout: Option<u64>,
    telemetry_enabled: bool,
    profile_name: &str,
    command_start_time: chrono::DateTime<chrono::Utc>,
    tilt_up_time: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    use devdock_monitor::{StartupMonitor, StartupMonitorConfig, StartupResult};
    let instance_id = instance_id.to_string();
    let workspace_root = workspace_root.clone();
    let profile_name = profile_name.to_string();

    // Track watch start time (when monitoring begins)
    let watch_start_time = chrono::Utc::now();

    // Get PID before moving the process
    let tilt_pid = tilt_process.id();

    // Convert std Child to tokio Command for async waiting
    let mut tilt_handle = tokio::task::spawn_blocking(move || tilt_process.wait());

    // If monitoring is enabled, run the startup monitor
    if run_monitor {
        use devdock_monitor::StabilityConfig;
        use std::time::Duration;

        let monitor_config = StartupMonitorConfig {
            timeout_secs: monitor_timeout.unwrap_or(1200),
            poll_interval_secs: 2,
            notifications_enabled: true,
            show_progress: true,
            skip_if_completed: false, // Don't skip when integrated with start
            collect_stats: true,
            stability: StabilityConfig {
                required_successes: 3,
                window_duration: Duration::from_secs(10),
                min_healthy_duration: Duration::from_secs(5),
            },
        };

        let monitor = StartupMonitor::new(monitor_config);

        tokio::select! {
            result = monitor.run() => {
                match &result {
                    Ok(StartupResult::Success { duration_secs, resources }) => {
                        output::success(&format!(
                            "All {} resources healthy in {}s - monitoring complete",
                            resources.len(),
                            duration_secs
                        ));

                        if telemetry_enabled {
                            emit_telemetry(
                                &instance_id,
                                "success",
                                resources.len(),
                                0,
                                &profile_name,
                                command_start_time,
                                tilt_up_time,
                                watch_start_time,
                            ).await;
                        }
                    }
                    Ok(StartupResult::Timeout { healthy, pending, failed, .. }) => {
                        output::warn(&format!(
                            "Startup monitoring timed out: {} healthy, {} pending, {} failed",
                            healthy.len(), pending.len(), failed.len()
                        ));

                        if telemetry_enabled {
                            emit_telemetry(
                                &instance_id,
                                "timeout",
                                healthy.len(),
                                failed.len(),
                                &profile_name,
                                command_start_time,
                                tilt_up_time,
                                watch_start_time,
                            ).await;
                        }
                    }
                    Ok(StartupResult::Failed { failed, .. }) => {
                        output::warn(&format!(
                            "Startup monitoring failed: {} resource(s) in error state",
                            failed.len()
                        ));

                        if telemetry_enabled {
                            emit_telemetry(
                                &instance_id,
                                "failed",
                                0,
                                failed.len(),
                                &profile_name,
                                command_start_time,
                                tilt_up_time,
                                watch_start_time,
                            ).await;
                        }
                    }
                    Ok(StartupResult::TiltNotRunning) => {
                        output::warn("Tilt stopped during monitoring");

                        if telemetry_enabled {
                            emit_telemetry(
                                &instance_id,
                                "tilt_stopped",
                                0,
                                0,
                                &profile_name,
                                command_start_time,
                                tilt_up_time,
                                watch_start_time,
                            ).await;
                        }
                    }
                    Err(e) => {
                        output::warn(&format!("Monitor error: {e}"));
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                output::info("Received shutdown signal during monitoring...");
            }
        }

        println!("\nPress Ctrl+C to stop DevDock\n");
    }

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
                output::info(&format!("Terminating Tilt process: {tilt_pid}"));
                // Send SIGTERM for graceful shutdown
                unsafe {
                    libc::kill(tilt_pid as i32, libc::SIGTERM);
                }

                // Wait briefly for graceful shutdown (reduced from 2s to 500ms)
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

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
                        output::warn(&format!("Tilt exited with status: {status}"));

                        // Emit telemetry for tilt crash
                        if telemetry_enabled {
                            emit_tilt_failure_telemetry(
                                &instance_id,
                                &profile_name,
                                "tilt_crashed",
                                command_start_time,
                            ).await;
                        }
                    }
                }
                Ok(Err(e)) => {
                    output::warn(&format!("Tilt process error: {e}"));

                    // Emit telemetry for tilt error
                    if telemetry_enabled {
                        emit_tilt_failure_telemetry(
                            &instance_id,
                            &profile_name,
                            "tilt_error",
                            command_start_time,
                        ).await;
                    }

                    perform_cleanup(&instance_id, &workspace_root).await?;
                    state_manager
                        .update_status(&instance_id, InstanceStatus::Stopped)
                        .await?;
                }
                Err(e) => {
                    output::warn(&format!("Failed to wait for Tilt: {e}"));
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

    // 1. Stop Docker containers (fast with batch command)
    cleanup_docker_containers()?;

    // 2. Kill processes on ports (fast without sleeps)
    cleanup_port_processes()?;

    // 3. Clean up dangling images (optimized batch command)
    cleanup_dangling_images()?;

    // 4. Clean up temporary files (fast)
    cleanup_temp_files(workspace_root)?;

    output::success("Cleanup complete");
    Ok(())
}

/// Stop all DevDock Docker containers
fn cleanup_docker_containers() -> Result<()> {
    output::info("Stopping Docker containers...");

    // Use batch docker command for much faster cleanup
    // This stops and removes all devdock containers in one command
    #[cfg(unix)]
    {
        let status = Command::new("sh")
            .arg("-c")
            .arg("docker ps -a --filter 'name=devdock-' --format '{{.Names}}' | xargs -r docker rm -f 2>/dev/null || true")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        if let Ok(status) = status {
            if status.success() {
                output::success("Stopped Docker containers");
            }
        }
    }

    #[cfg(not(unix))]
    {
        // Fallback for non-Unix systems
        let output = Command::new("docker")
            .args(["ps", "-a", "--filter", "name=devdock-", "-q"])
            .output()?;

        if output.status.success() {
            let container_ids = String::from_utf8_lossy(&output.stdout);
            if !container_ids.trim().is_empty() {
                Command::new("docker")
                    .arg("rm")
                    .arg("-f")
                    .args(container_ids.lines())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .ok();
                output::success("Stopped Docker containers");
            }
        }
    }

    Ok(())
}

/// Kill processes on specific ports
fn cleanup_port_processes() -> Result<()> {
    output::info("Cleaning up processes on ports...");

    let ports = vec![8888, 8880, 7070, 8090, 7972, 3407, 3408, 10350];
    let mut killed = 0;

    #[cfg(unix)]
    {
        // Collect all PIDs first to kill them in batch
        let mut pids_to_kill = Vec::new();

        for port in ports {
            let output = Command::new("lsof").args(["-i", &format!(":{port}"), "-t"]).output();

            if let Ok(output) = output {
                if output.status.success() {
                    let pids = String::from_utf8_lossy(&output.stdout);
                    for pid_str in pids.lines() {
                        if let Ok(pid) = pid_str.parse::<i32>() {
                            pids_to_kill.push(pid);
                        }
                    }
                }
            }
        }

        // Kill all processes at once with SIGKILL (fast)
        for pid in pids_to_kill {
            unsafe {
                if libc::kill(pid, libc::SIGKILL) == 0 {
                    killed += 1;
                }
            }
        }
    }

    if killed > 0 {
        output::success(&format!("Stopped {killed} process(es) on ports"));
    }

    Ok(())
}

/// Remove dangling Docker images (optimized batch version)
fn cleanup_dangling_images() -> Result<()> {
    output::info("Cleaning up dangling images...");

    #[cfg(unix)]
    {
        // Use batch command for fast cleanup
        let status = Command::new("sh")
            .arg("-c")
            .arg("docker images --filter 'dangling=true' -q | xargs -r docker rmi 2>/dev/null || true")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        if let Ok(status) = status {
            if status.success() {
                output::success("Cleaned up dangling images");
            }
        }
    }

    #[cfg(not(unix))]
    {
        // Fallback for non-Unix systems
        let output =
            Command::new("docker").args(["images", "--filter", "dangling=true", "-q"]).output()?;

        if output.status.success() {
            let images = String::from_utf8_lossy(&output.stdout);
            let image_list: Vec<&str> = images.lines().filter(|s| !s.is_empty()).collect();

            if !image_list.is_empty() {
                Command::new("docker")
                    .arg("rmi")
                    .args(&image_list)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .ok();
                output::success(&format!("Removed {} dangling image(s)", image_list.len()));
            }
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

    // Also clean up config files
    let config_files = vec![
        workspace_root.join("devdock/devdock.ini"),
        workspace_root.join("config/devdock.ini"),
        workspace_root.join("config/run_local.ini"),
        workspace_root.join("devdock/scio_apps.ini"),
    ];

    for config_file in config_files {
        if config_file.exists() && std::fs::remove_file(&config_file).is_ok() {
            cleaned_count += 1;
        }
    }

    output::success(&format!("Cleaned {cleaned_count} file(s)/directory(s)"));
    Ok(())
}

/// Emit telemetry data to BigQuery
#[allow(clippy::too_many_arguments)]
async fn emit_telemetry(
    devdock_id: &str,
    status: &str,
    healthy_count: usize,
    failed_count: usize,
    profile: &str,
    command_start_time: chrono::DateTime<chrono::Utc>,
    tilt_up_time: chrono::DateTime<chrono::Utc>,
    watch_start_time: chrono::DateTime<chrono::Utc>,
) {
    use chrono::Utc;
    use devdock_telemetry::{
        BigQueryConfig, BigQueryEmitter, SessionMetrics, TelemetryEmitter, TelemetrySnapshot,
    };

    let machine_id = get_machine_id();
    let now = Utc::now();

    // Calculate timing metrics
    let tilt_bootstrap_secs = (tilt_up_time - command_start_time).num_seconds() as f64;
    let total_startup_secs = (now - command_start_time).num_seconds() as f64;

    let session = SessionMetrics {
        devdock_id: devdock_id.to_string(),
        machine_id: machine_id.clone(),
        architecture: std::env::consts::OS.to_string(),
        platform: std::env::consts::OS.to_string(),
        project: "scio".to_string(),
        profile: profile.to_string(),
        branch: get_git_branch().ok(),
        binary_type: "rust".to_string(),
        binary_version: env!("CARGO_PKG_VERSION").to_string(),
        command_start_time,
        tilt_up_time: Some(tilt_up_time),
        watch_start_time: Some(watch_start_time),
        startup_complete_time: Some(now),
        tilt_bootstrap_seconds: Some(tilt_bootstrap_secs),
        total_startup_seconds: Some(total_startup_secs),
        startup_status: status.to_string(),
        total_resources: (healthy_count + failed_count) as i32,
        healthy_resources: healthy_count as i32,
        failed_resources: failed_count as i32,
        timestamp: now,
    };

    let snapshot = TelemetrySnapshot { session: Some(session), ..Default::default() };

    let config = BigQueryConfig {
        project_id: "dev-sandbox-334901".to_string(),
        dataset: "devdock".to_string(),
        use_adc: true,
        ..Default::default()
    };

    match BigQueryEmitter::new(config) {
        Ok(emitter) => match emitter.emit(&snapshot).await {
            Ok(result) => {
                if result.is_success() {
                    tracing::info!("Telemetry emitted to BigQuery: status={}", status);
                } else {
                    tracing::warn!("Partial telemetry emission: {} errors", result.failure_count);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to emit telemetry to BigQuery: {}", e);
            }
        },
        Err(e) => {
            tracing::debug!(
                "BigQuery telemetry not available: {} (this is normal for local dev)",
                e
            );
        }
    }
}

/// Emit telemetry when Tilt fails early (before monitor runs)
async fn emit_tilt_failure_telemetry(
    devdock_id: &str,
    profile: &str,
    failure_type: &str,
    command_start_time: chrono::DateTime<chrono::Utc>,
) {
    use chrono::Utc;
    use devdock_telemetry::{
        BigQueryConfig, BigQueryEmitter, SessionMetrics, TelemetryEmitter, TelemetrySnapshot,
    };

    let machine_id = get_machine_id();
    let now = Utc::now();
    let total_startup_secs = (now - command_start_time).num_seconds() as f64;

    let session = SessionMetrics {
        devdock_id: devdock_id.to_string(),
        machine_id: machine_id.clone(),
        architecture: std::env::consts::OS.to_string(),
        platform: std::env::consts::OS.to_string(),
        project: "scio".to_string(),
        profile: profile.to_string(),
        branch: get_git_branch().ok(),
        binary_type: "rust".to_string(),
        binary_version: env!("CARGO_PKG_VERSION").to_string(),
        command_start_time,
        tilt_up_time: None,
        watch_start_time: None,
        startup_complete_time: Some(now),
        tilt_bootstrap_seconds: None,
        total_startup_seconds: Some(total_startup_secs),
        startup_status: failure_type.to_string(),
        total_resources: 0,
        healthy_resources: 0,
        failed_resources: 0,
        timestamp: now,
    };

    let snapshot = TelemetrySnapshot { session: Some(session), ..Default::default() };

    // Configure BigQuery emitter
    let config = BigQueryConfig {
        project_id: "dev-sandbox-334901".to_string(),
        dataset: "devdock".to_string(),
        use_adc: true,
        ..Default::default()
    };

    // Try to emit to BigQuery
    match BigQueryEmitter::new(config) {
        Ok(emitter) => match emitter.emit(&snapshot).await {
            Ok(result) => {
                if result.is_success() {
                    tracing::info!("Tilt failure telemetry emitted: {}", failure_type);
                }
            }
            Err(e) => {
                tracing::debug!("Failed to emit failure telemetry: {}", e);
            }
        },
        Err(e) => {
            tracing::debug!("BigQuery not available: {}", e);
        }
    }
}

/// Get a stable machine identifier
fn get_machine_id() -> String {
    use std::ffi::CStr;

    #[cfg(unix)]
    {
        let mut name = [0 as libc::c_char; 256];
        unsafe {
            if libc::gethostname(name.as_mut_ptr(), 256) == 0 {
                if let Ok(hostname) = CStr::from_ptr(name.as_ptr()).to_str() {
                    return hostname.to_string();
                }
            }
        }
    }

    // Fallback to USER env var
    std::env::var("USER").unwrap_or_else(|_| "unknown".to_string())
}

/// Write current profile name to state file for telemetry
fn write_profile_state(profile_name: &str) -> Result<()> {
    let home = std::env::var("HOME")?;
    let state_file = PathBuf::from(home).join(".devdock/current_profile.txt");

    if let Some(parent) = state_file.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(&state_file, profile_name)?;
    Ok(())
}
