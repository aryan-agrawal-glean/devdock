//! Start command implementation.
//!
//! Uses the lifecycle state machine to drive startup through well-defined states.

use crate::output;
use anyhow::{Context, Result};
use colored::Colorize;
use devdock_config::{DependencyResolver, ProfileLoader, ProfileValidator};
use devdock_core::PluginManager;
use devdock_lifecycle::{LifecycleConfig, LifecycleMachine, LifecycleOutput};
use devdock_state::{Instance, InstanceStatus, PortAllocator, SessionOrigin, StateManager};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// Adapter to bridge crate::output to LifecycleOutput trait.
struct CliOutput;

impl LifecycleOutput for CliOutput {
    fn info(&self, msg: &str) {
        output::info(msg);
    }
    fn success(&self, msg: &str) {
        output::success(msg);
    }
    fn warn(&self, msg: &str) {
        output::warn(msg);
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    profile_name: &str,
    services: Option<&str>,
    ci_mode: bool,
    run_monitor: bool,
    monitor_timeout: Option<u64>,
    telemetry_enabled: bool,
    dry_run: bool,
    _enable_logs: bool,
) -> Result<()> {
    // Track command start time for telemetry
    let command_start_time = chrono::Utc::now();

    output::info("Starting DevDock...");

    // ── State: Initializing ──────────────────────────────────────────────
    // Step 1: Load and validate profile
    output::info(&format!("Loading profile: {profile_name}"));
    let loader = ProfileLoader::new();
    let (profile, _profile_path) = loader.load_with_path(profile_name)?;

    let validator = ProfileValidator::new();
    validator.validate(&profile)?;

    // Skip gcloud project validation in dry-run mode (for benchmarking/CI)
    if !dry_run {
        validator.validate_gcloud_project(&profile)?;
    }
    output::success("Profile validated");

    // Step 2: Resolve dependencies
    let resolver = DependencyResolver::new();
    let resolved_components = resolver.resolve(&profile)?;
    output::info(&format!("Resolved {} components", resolved_components.len()));

    // Find workspace root
    let workspace_root = find_workspace_root()?;

    // Load devdock.toml and create plugin manager
    let devdock_toml_path = workspace_root.join(".devdock/devdock.toml");
    if !devdock_toml_path.exists() {
        anyhow::bail!(
            "Missing .devdock/devdock.toml — run `devdock init` to initialize your project"
        );
    }
    let project_config = devdock_core::ProjectConfig::load(&devdock_toml_path)
        .context("Failed to load .devdock/devdock.toml")?;
    project_config.export_diagnostics_env();

    // Set environment variables from devdock.toml for use by Starlark libs and infra Tiltfiles
    let platform_str = project_config.cloud.platform.to_uppercase();
    std::env::set_var("CLOUD_PLATFORM", &platform_str);
    std::env::set_var("DEVDOCK_SERVICES_DIR", &project_config.project.services_dir);
    std::env::set_var("DEVDOCK_INFRA_DIR", &project_config.project.infra_dir);

    // Initialize PluginManager with lifecycle hooks + resource hooks
    let plugin_manager = PluginManager::with_resource_hooks(
        project_config.plugins.hooks.clone(),
        project_config.plugins.resource_hooks.clone(),
    );

    // Create lifecycle machine
    let lifecycle_config = LifecycleConfig {
        profile_name: profile_name.to_string(),
        services: services.map(String::from),
        ci_mode,
        run_monitor,
        monitor_timeout,
        telemetry_enabled,
        dry_run,
        workspace_root: workspace_root.clone(),
    };

    let mut machine = LifecycleMachine::new(lifecycle_config, plugin_manager, Box::new(CliOutput));
    machine.set_platform(project_config.cloud.platform.clone());
    machine.set_project_name(project_config.project.name.clone());

    // ── State: ProfileLoaded ─────────────────────────────────────────────
    machine.advance()?; // Initializing -> ProfileLoaded
    machine.fire_after_profile_loaded();

    // Initialize plugins
    if machine.has_plugins() {
        output::info("Initializing plugins...");
        let plugin_config = serde_json::json!({
            "project": project_config.project.name,
            "workspace_root": workspace_root.to_string_lossy(),
            "platform": project_config.cloud.platform,
            "profile": profile_name,
        });
        machine.plugin_manager_mut().initialize_all(&plugin_config)?;
        output::success("Plugins initialized");
    }

    // ── State: PluginsReady ──────────────────────────────────────────────
    machine.advance()?; // ProfileLoaded -> PluginsReady

    // HOOK: before_infra_ready — auth, cloud credential setup
    machine.fire_before_infra_ready();

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
        (None, Instance::generate_id(origin), 0)
    };

    // Set DEVDOCK_ID early so config generation can inject it into service env vars
    if !instance_id.is_empty() {
        std::env::set_var("DEVDOCK_ID", &instance_id);
    }
    machine.set_instance_id(instance_id.clone());
    machine.set_port_base(_port_base);

    // ── State: InfraReady ────────────────────────────────────────────────
    machine.advance()?; // PluginsReady -> InfraReady

    // HOOK: before_config_generated — fetch remote configs
    machine.fire_before_config_generated();

    // Step 6b: Generate configuration files
    // Always generate minimal temp_profile.yaml as baseline.
    // Plugins can enrich it via after_config_generated hook.
    generate_minimal_temp_profile(&workspace_root, &profile, services, dry_run).await?;

    // ── State: ConfigReady ───────────────────────────────────────────────
    machine.advance()?; // InfraReady -> ConfigReady

    // HOOK: after_config_generated — write devdock.ini, enrich configs
    machine.fire_after_config_generated();

    // Step 6c: Generate OSS Tiltfile
    generate_oss_tiltfile(&workspace_root, &profile)?;

    // Step 7: Clean old logs and startup state files
    cleanup_old_logs()?;
    cleanup_startup_state_files()?;

    // ── State: TiltfileReady ─────────────────────────────────────────────
    machine.advance()?; // ConfigReady -> TiltfileReady

    // If dry-run mode, exit here with timing information
    if dry_run {
        let elapsed = chrono::Utc::now() - command_start_time;
        let elapsed_ms = elapsed.num_milliseconds();

        println!("{{\"profile\": \"{}\", \"bootstrap_time_ms\": {}}}", profile_name, elapsed_ms);

        output::success(&format!("Dry-run completed in {}ms", elapsed_ms));
        std::process::exit(0);
    }

    // Unwrap state_manager (safe because dry_run is false)
    let state_manager = state_manager.unwrap();

    // HOOK: before_tilt — last-minute config enrichment
    machine.fire_before_tilt();

    // Step 8: Update status and start Tilt
    state_manager.update_status(&instance_id, InstanceStatus::Running).await?;
    write_profile_state(profile_name)?;

    output::info("Starting Tilt...");
    let tilt_process = start_tilt(services, profile_name)?;

    let tilt_up_time = chrono::Utc::now();

    // ── State: TiltRunning ───────────────────────────────────────────────
    machine.advance()?; // TiltfileReady -> TiltRunning

    // HOOK: after_tilt — telemetry: session started
    machine.fire_after_tilt();

    let gcp_logs_url = format!(
        "https://console.cloud.google.com/logs/query;query=labels.devdock_id%3D%22{instance_id}%22;summaryFields=labels%252Flogger_source:false:32:beginning?project={}",
        profile.project
    );

    output::success("DevDock started!");
    println!();
    println!("  {}", "──────────────────────────────────────────────────────────".cyan());
    println!("  {} http://localhost:10350", "Tilt UI:".cyan().bold());
    println!("  {} {}", "Session:".cyan().bold(), instance_id.dimmed());
    println!("  {} {}", "Logs:   ".cyan().bold(), gcp_logs_url.underline());
    println!("  {}", "──────────────────────────────────────────────────────────".cyan());

    // ── State: ResourcesStarting ─────────────────────────────────────────
    machine.advance()?; // TiltRunning -> ResourcesStarting

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
        &mut machine,
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

    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let result =
            Command::new("docker").arg("info").stdout(Stdio::null()).stderr(Stdio::null()).status();
        tx.send(result).ok();
    });

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

/// Generate a minimal temp_profile.yaml with service names, types, and env vars.
async fn generate_minimal_temp_profile(
    workspace_root: &Path,
    profile: &devdock_core::profile::Profile,
    services: Option<&str>,
    _dry_run: bool,
) -> Result<()> {
    let runtime_dir = workspace_root.join("devdock/runtime_generated");
    std::fs::create_dir_all(&runtime_dir)?;

    let mut profile_map = serde_yaml::Mapping::new();
    let devdock_id = std::env::var("DEVDOCK_ID").unwrap_or_default();

    // Include top-level fields expected by infra Tiltfiles
    profile_map.insert(
        serde_yaml::Value::String("project".into()),
        serde_yaml::Value::String(profile.project.clone()),
    );
    // Platform and region come from CLOUD_PLATFORM env var (set earlier in startup)
    // and the project config's cloud section
    let platform_str =
        std::env::var("CLOUD_PLATFORM").unwrap_or_else(|_| "none".into()).to_lowercase();
    profile_map.insert(
        serde_yaml::Value::String("platform".into()),
        serde_yaml::Value::String(platform_str),
    );
    // Region from env var or project config or default
    let region = std::env::var("GOOGLE_CLOUD_REGION")
        .or_else(|_| std::env::var("AWS_REGION"))
        .unwrap_or_else(|_| "us-central1".into());
    profile_map
        .insert(serde_yaml::Value::String("region".into()), serde_yaml::Value::String(region));

    let service_filter: Option<Vec<&str>> = services.map(|s| s.split(',').map(str::trim).collect());

    // Build services section with env vars and active flag
    let mut services_map = serde_yaml::Mapping::new();
    for (name, svc) in &profile.services {
        let is_active = if let Some(ref filter) = service_filter {
            filter.contains(&name.as_str())
        } else {
            true
        };

        let mut svc_entry = serde_yaml::Mapping::new();
        svc_entry.insert(
            serde_yaml::Value::String("type".into()),
            serde_yaml::to_value(svc.deployment_type)
                .unwrap_or(serde_yaml::Value::String("local".into())),
        );
        svc_entry.insert(
            serde_yaml::Value::String("active".into()),
            serde_yaml::Value::String(is_active.to_string()),
        );

        let mut env_map = serde_yaml::Mapping::new();
        env_map.insert(
            serde_yaml::Value::String("DEVDOCK_ID".into()),
            serde_yaml::Value::String(devdock_id.clone()),
        );
        env_map.insert(
            serde_yaml::Value::String("IS_DEVDOCK".into()),
            serde_yaml::Value::String("true".into()),
        );

        // Merge env vars from profile
        for (k, v) in &svc.env {
            env_map
                .insert(serde_yaml::Value::String(k.clone()), serde_yaml::Value::String(v.clone()));
        }

        svc_entry
            .insert(serde_yaml::Value::String("env".into()), serde_yaml::Value::Mapping(env_map));

        services_map
            .insert(serde_yaml::Value::String(name.clone()), serde_yaml::Value::Mapping(svc_entry));
    }

    profile_map.insert(
        serde_yaml::Value::String("services".into()),
        serde_yaml::Value::Mapping(services_map),
    );

    // Build infra section from profile
    let mut infra_map = serde_yaml::Mapping::new();
    for (name, infra) in &profile.infra {
        let infra_value = serde_yaml::to_value(infra)
            .unwrap_or(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
        infra_map.insert(serde_yaml::Value::String(name.clone()), infra_value);
    }

    profile_map
        .insert(serde_yaml::Value::String("infra".into()), serde_yaml::Value::Mapping(infra_map));

    let yaml = serde_yaml::to_string(&profile_map)?;
    std::fs::write(runtime_dir.join("temp_profile.yaml"), &yaml)?;

    output::success("Generated minimal temp_profile.yaml");
    Ok(())
}

/// Generate Tiltfile for OSS pipeline.
fn generate_oss_tiltfile(
    workspace_root: &Path,
    profile: &devdock_core::profile::Profile,
) -> Result<()> {
    use devdock_core::project_config::ProjectConfig;
    use devdock_core::resolver;
    use devdock_core::service_config::discover_services;
    use devdock_core::tiltfile_generator;

    let config_path = workspace_root.join(".devdock/devdock.toml");
    let project_config =
        ProjectConfig::load(&config_path).context("Failed to load .devdock/devdock.toml")?;

    let services_dir = workspace_root.join(&project_config.project.services_dir);
    let service_configs =
        discover_services(&services_dir).context("Failed to discover service configs")?;

    output::info(&format!("Discovered {} service config(s)", service_configs.len()));

    let mut requested: Vec<String> = profile.services.keys().cloned().collect();
    requested.extend(profile.infra.keys().cloned());

    let resolved = resolver::resolve(&requested, &service_configs)
        .context("Failed to resolve dependencies")?;

    output::info(&format!(
        "Resolved: {} service(s), {} infra component(s)",
        resolved.services.len(),
        resolved.infra.len()
    ));

    let tiltfile = tiltfile_generator::generate_tiltfile(
        &service_configs,
        &resolved,
        &project_config,
        workspace_root,
    );

    let runtime_dir = workspace_root.join("devdock/runtime_generated");
    std::fs::create_dir_all(&runtime_dir)?;
    std::fs::write(runtime_dir.join("Tiltfile"), &tiltfile)?;

    output::success("Generated OSS Tiltfile");
    Ok(())
}

/// Clean up old logs
fn cleanup_old_logs() -> Result<()> {
    output::info("Cleaning up old logs...");

    let home = std::env::var("HOME")?;
    let devdock_dir = PathBuf::from(&home).join(".devdock");

    if devdock_dir.exists() {
        for entry in std::fs::read_dir(&devdock_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let dir_name = path.file_name().unwrap().to_string_lossy();

                if matches!(
                    dir_name.as_ref(),
                    "images" | "auth" | "sql" | "grafana" | "bazel" | "generated" | "sync"
                ) {
                    continue;
                }

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

/// Start Tilt process using the generated Tiltfile.
fn start_tilt(_services: Option<&str>, profile_name: &str) -> Result<Child> {
    let workspace_root = find_workspace_root()?;

    std::env::set_var("DEVDOCK_ROOT", &workspace_root);
    std::env::set_var("DEVDOCK_RUST_BINARY", "true");
    std::env::set_var("DEVDOCK_PROFILE", profile_name);

    devdock_core::tiltfile_generator::extract_shipped_libs_to(&workspace_root)
        .context("Failed to extract shipped Starlark libraries")?;

    let runtime_dir = workspace_root.join("devdock/runtime_generated");
    let mut cmd = Command::new("tilt");
    cmd.arg("up");
    cmd.current_dir(&runtime_dir);

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
                 Are you running this from inside a project with a devdock/ directory?"
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
    machine: &mut LifecycleMachine,
) -> Result<()> {
    use devdock_monitor::{StartupMonitor, StartupMonitorConfig, StartupResult};
    let instance_id = instance_id.to_string();
    let workspace_root = workspace_root.clone();
    let profile_name = profile_name.to_string();

    let watch_start_time = chrono::Utc::now();
    let tilt_pid = tilt_process.id();
    let mut tilt_handle = tokio::task::spawn_blocking(move || tilt_process.wait());

    if run_monitor {
        use devdock_monitor::StabilityConfig;
        use std::time::Duration;

        let monitor_config = StartupMonitorConfig {
            timeout_secs: monitor_timeout.unwrap_or(1200),
            poll_interval_secs: 2,
            notifications_enabled: true,
            show_progress: true,
            skip_if_completed: false,
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

                        // ── State: Running (all resources healthy) ───────
                        let _ = machine.advance(); // ResourcesStarting -> Running
                        machine.fire_after_ready(
                            resources.len(),
                            *duration_secs as f64,
                        );

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

            // ── State: Stopping ──────────────────────────────────────────
            let _ = machine.advance(); // Running/ResourcesStarting -> Stopping

            // HOOK: before_stop — final telemetry flush
            machine.fire_before_stop();

            state_manager
                .update_status(&instance_id, InstanceStatus::Stopping)
                .await
                .ok();

            // Kill Tilt process
            #[cfg(unix)]
            {
                output::info(&format!("Terminating Tilt process: {tilt_pid}"));
                unsafe {
                    libc::kill(tilt_pid as i32, libc::SIGTERM);
                }

                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                if unsafe { libc::kill(tilt_pid as i32, 0) } == 0 {
                    unsafe {
                        libc::kill(tilt_pid as i32, libc::SIGKILL);
                    }
                }
            }

            machine.shutdown_plugins();

            // Perform cleanup
            perform_cleanup(&instance_id, &workspace_root).await?;

            // ── State: Stopped ───────────────────────────────────────────
            state_manager
                .update_status(&instance_id, InstanceStatus::Stopped)
            .await
            .ok();

            // HOOK: after_stop — post-cleanup actions
            machine.fire_after_stop();

            output::success("DevDock stopped successfully");
        }
        result = &mut tilt_handle => {
            match result {
                Ok(Ok(status)) => {
                    perform_cleanup(&instance_id, &workspace_root).await?;

                    state_manager
                        .update_status(&instance_id, InstanceStatus::Stopped)
                        .await?;

                    if status.success() {
                        output::success("DevDock stopped");
                    } else {
                        output::warn(&format!("Tilt exited with status: {status}"));

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

    cleanup_docker_containers()?;
    cleanup_port_processes()?;
    cleanup_dangling_images()?;
    cleanup_temp_files(workspace_root)?;

    output::success("Cleanup complete");
    Ok(())
}

/// Stop all DevDock Docker containers
fn cleanup_docker_containers() -> Result<()> {
    output::info("Stopping Docker containers...");

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

    output::info(&format!("Workspace root: {}", workspace_root.display()));

    let runtime_gen = workspace_root.join("devdock/runtime_generated");
    if runtime_gen.exists() {
        match std::fs::remove_dir_all(&runtime_gen) {
            Ok(()) => {
                output::success("Removed runtime_generated directory");
                cleaned_count += 1;
            }
            Err(e) => {
                output::warn(&format!("Failed to remove runtime_generated: {e}"));
            }
        }
    }

    // Clean up devdock/core/ (symlinks + empty parent dir)
    for link in &["devdock/core/lib", "devdock/core/templates"] {
        let path = workspace_root.join(link);
        if path.is_symlink() || path.exists() {
            let _ = std::fs::remove_file(&path).or_else(|_| std::fs::remove_dir_all(&path));
            cleaned_count += 1;
        }
    }
    let core_dir = workspace_root.join("devdock/core");
    if core_dir.exists() {
        let _ = std::fs::remove_dir(&core_dir);
    }

    // Clean up devdock/devdock.ini
    let ini_path = workspace_root.join("devdock/devdock.ini");
    if ini_path.exists() && std::fs::remove_file(&ini_path).is_ok() {
        cleaned_count += 1;
    }

    // Clean up generated Dockerfiles and docker-compose.yaml from service directories
    let services_dir =
        std::env::var("DEVDOCK_SERVICES_DIR").unwrap_or_else(|_| "devdock/services".into());
    let services_path = workspace_root.join(&services_dir);
    if services_path.exists() {
        if let Ok(entries) = std::fs::read_dir(&services_path) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    for file in &["Dockerfile", "docker-compose.yaml"] {
                        let generated = entry.path().join(file);
                        if generated.exists() && std::fs::remove_file(&generated).is_ok() {
                            cleaned_count += 1;
                        }
                    }
                }
            }
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
    _tilt_up_time: chrono::DateTime<chrono::Utc>,
    _watch_start_time: chrono::DateTime<chrono::Utc>,
) {
    let now = chrono::Utc::now();
    let total_startup_secs = (now - command_start_time).num_seconds() as f64;

    use devdock_telemetry::{SessionMetrics, TelemetryEmitter, TelemetrySnapshot};

    let session = SessionMetrics {
        devdock_id: devdock_id.to_string(),
        machine_id: get_machine_id(),
        architecture: std::env::consts::OS.to_string(),
        platform: std::env::consts::OS.to_string(),
        project: std::env::var("DEVDOCK_PROJECT").unwrap_or_default(),
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
        startup_status: status.to_string(),
        total_resources: (healthy_count + failed_count) as i32,
        healthy_resources: healthy_count as i32,
        failed_resources: failed_count as i32,
        timestamp: now,
    };

    let snapshot = TelemetrySnapshot { session: Some(session), ..Default::default() };

    let emitter = devdock_telemetry::FileEmitter::new("~/.devdock/telemetry");
    if let Err(e) = emitter.emit(&snapshot).await {
        tracing::debug!("File telemetry: {e}");
    }
}

/// Emit telemetry when Tilt fails early (before monitor runs)
async fn emit_tilt_failure_telemetry(
    devdock_id: &str,
    profile: &str,
    failure_type: &str,
    command_start_time: chrono::DateTime<chrono::Utc>,
) {
    emit_telemetry(
        devdock_id,
        failure_type,
        0,
        0,
        profile,
        command_start_time,
        chrono::Utc::now(),
        chrono::Utc::now(),
    )
    .await;
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
