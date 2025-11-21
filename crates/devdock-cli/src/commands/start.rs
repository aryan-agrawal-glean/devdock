//! Start command implementation.

use crate::output;
use anyhow::{Context, Result};
use devdock_codegen::TiltfileGenerator;
use devdock_config::{DependencyResolver, ProfileLoader, ProfileValidator};
use devdock_state::{Instance, InstanceStatus, PortAllocator, StateManager};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub async fn execute(profile_name: &str, services: Option<&str>, ci_mode: bool) -> Result<()> {
    output::info("Starting DevDock...");

    // Step 1: Load and validate profile
    output::info(&format!("Loading profile: {}", profile_name));
    let loader = ProfileLoader::new();
    let profile = loader.load(profile_name)?;

    let validator = ProfileValidator::new();
    validator.validate(&profile)?;
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

    // Step 6: Generate configuration files
    let gen_dir = generate_configs(&profile, profile_name, &resolved_components, services)?;
    output::success(&format!("Generated configs in: {}", gen_dir.display()));

    // Step 7: Clean old logs
    cleanup_old_logs()?;

    // Step 8: Update status and start Tilt
    state_manager
        .update_status(&instance.id, InstanceStatus::Running)
        .await?;

    output::info("Starting Tilt...");
    let tilt_process = start_tilt(&gen_dir, services)?;

    output::success("DevDock started!");
    println!("\n  Tilt UI: http://localhost:10350");
    println!("  Services: https://local.glean.com:{}", port_base);
    println!("\nPress Ctrl+C to stop DevDock\n");

    // Wait for Tilt process
    wait_for_tilt(tilt_process, &state_manager, &instance.id).await?;

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

/// Generate configuration files
fn generate_configs(
    profile: &devdock_core::profile::Profile,
    _profile_name: &str,
    _resolved_components: &[String],
    _services: Option<&str>,
) -> Result<PathBuf> {
    output::info("Generating configuration files...");

    // Get current directory (should be in monorepo)
    let current_dir = std::env::current_dir()?;
    
    // Try to find workspace root from current location
    let workspace_root = find_workspace_root_from(&current_dir)?;
    
    // Generate in monorepo's runtime_generated directory (where Tilt expects it)
    let runtime_gen_dir = workspace_root.join("devdock/runtime_generated");
    std::fs::create_dir_all(&runtime_gen_dir)?;

    // Generate temp_profile.yaml where Tilt expects it
    let temp_profile_path = runtime_gen_dir.join("temp_profile.yaml");
    
    // Create an enriched profile with additional fields Tilt expects
    let temp_profile = generate_temp_profile(&profile, _services, &workspace_root)?;
    std::fs::write(&temp_profile_path, temp_profile)?;
    
    // Generate placeholder watch files for all services
    // This avoids Python dependency for cquery.py
    generate_watch_files(&profile, &runtime_gen_dir, &workspace_root)?;

    // Generate devdock.ini in monorepo devdock directory
    let devdock_ini_path = workspace_root.join("devdock/devdock.ini");
    let ini_content = generate_devdock_ini(profile)?;
    std::fs::write(&devdock_ini_path, &ini_content)?;

    // Also generate in config root (some services expect it here)
    let config_ini_path = workspace_root.join("config/devdock.ini");
    std::fs::write(&config_ini_path, &ini_content)?;

    // Also generate run_local.ini in config root
    let run_local_ini_path = workspace_root.join("config/run_local.ini");
    std::fs::write(&run_local_ini_path, &ini_content)?;

    output::success(&format!(
        "Generated configs in: {}",
        runtime_gen_dir.display()
    ));

    Ok(runtime_gen_dir)
}

/// Generate temp_profile.yaml with all fields Tilt expects
fn generate_temp_profile(
    profile: &devdock_core::profile::Profile,
    active_services: Option<&str>,
    workspace_root: &Path,
) -> Result<String> {
    let mut profile_map = serde_yaml::to_value(&profile)?;
    
    if let serde_yaml::Value::Mapping(ref mut map) = profile_map {
        // Add devdock_root
        map.insert(
            serde_yaml::Value::String("devdock_root".to_string()),
            serde_yaml::Value::String(workspace_root.display().to_string()),
        );
        
        // Add architecture (system type)
        let arch = if cfg!(target_os = "macos") {
            "darwin"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            "unknown"
        };
        map.insert(
            serde_yaml::Value::String("architecture".to_string()),
            serde_yaml::Value::String(arch.to_string()),
        );
        
        // Add enable_service_metrics
        map.insert(
            serde_yaml::Value::String("enable_service_metrics".to_string()),
            serde_yaml::Value::String("True".to_string()),
        );
        
        // Add region and zone (empty for now, will be populated by platform)
        map.insert(
            serde_yaml::Value::String("region".to_string()),
            serde_yaml::Value::String("".to_string()),
        );
        map.insert(
            serde_yaml::Value::String("zone".to_string()),
            serde_yaml::Value::String("".to_string()),
        );
        
        // Add "active" field to each service
        if let Some(services_value) = map.get_mut(&serde_yaml::Value::String("services".to_string())) {
            if let serde_yaml::Value::Mapping(ref mut services_map) = services_value {
                // Parse active services list
                let active_list: Vec<&str> = active_services
                    .map(|s| s.split(',').collect())
                    .unwrap_or_default();
                
                for (service_name, service_config) in services_map.iter_mut() {
                    if let serde_yaml::Value::Mapping(ref mut service_map) = service_config {
                        // Service is active if:
                        // 1. No specific services requested (all active)
                        // 2. Service is in active list
                        // 3. Service is remote (always active)
                        let is_remote = service_map
                            .get(&serde_yaml::Value::String("type".to_string()))
                            .and_then(|v| v.as_str())
                            == Some("remote");
                        
                        let service_name_str = service_name.as_str().unwrap_or("");
                        let is_active = active_services.is_none()
                            || active_list.contains(&service_name_str)
                            || is_remote;
                        
                        service_map.insert(
                            serde_yaml::Value::String("active".to_string()),
                            serde_yaml::Value::String(is_active.to_string()),
                        );
                    }
                }
            }
        }
    }
    
    serde_yaml::to_string(&profile_map)
        .context("Failed to serialize temp profile")
}

/// Generate devdock.ini file
fn generate_devdock_ini(profile: &devdock_core::profile::Profile) -> Result<String> {
    let mut ini = String::new();

    ini.push_str("[DEFAULT]\n");
    ini.push_str(&format!("project = {}\n", profile.project));
    ini.push_str(&format!("platform = {:?}\n", profile.platform));
    ini.push_str("\n");

    // Add service sections
    for (name, config) in &profile.services {
        ini.push_str(&format!("[service:{}]\n", name));
        ini.push_str(&format!("type = {:?}\n", config.deployment_type));
        for (key, value) in &config.env {
            ini.push_str(&format!("{} = {}\n", key, value));
        }
        ini.push_str("\n");
    }

    Ok(ini)
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

/// Start Tilt process
fn start_tilt(_gen_dir: &Path, services: Option<&str>) -> Result<Child> {
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

/// Generate watch files for Bazel hot reload
fn generate_watch_files(
    profile: &devdock_core::profile::Profile,
    runtime_gen_dir: &Path,
    workspace_root: &Path,
) -> Result<()> {
    // For each service, generate a watch file that tells Tilt what files to watch
    for (service_name, service_config) in &profile.services {
        if matches!(service_config.deployment_type, devdock_core::types::DeploymentType::Local) {
            let watch_file_path = runtime_gen_dir.join(format!("watch_file_{}.yaml", service_name));
            
            // Generate a comprehensive watch list based on service type
            // For now, generate a broad watch pattern that catches most changes
            let service_type = get_service_type(workspace_root, service_name)?;
            
            let watch_paths = match service_type.as_str() {
                "go" => vec![
                    format!("go/platform/gcp/{}", service_name),
                    format!("go/core"),
                    format!("go/platform/common"),
                ],
                "python" => vec![
                    format!("python_scio/{}", service_name),
                    format!("python_scio/core"),
                ],
                "java" => vec![
                    format!("java/com/askscio/{}", service_name),
                    format!("java/com/askscio/core"),
                ],
                _ => vec![
                    format!("go/{}", service_name),
                    format!("python_scio/{}", service_name),
                    format!("java/com/askscio/{}", service_name),
                ],
            };
            
            // Create YAML structure: { service_name: [paths...] }
            let mut watch_map = std::collections::HashMap::new();
            watch_map.insert(service_name.clone(), watch_paths);
            
            let watch_content = serde_yaml::to_string(&watch_map)?;
            std::fs::write(&watch_file_path, watch_content)?;
        }
    }
    
    Ok(())
}

/// Get service type from devdock_config.json
fn get_service_type(workspace_root: &Path, service_name: &str) -> Result<String> {
    let config_path = workspace_root
        .join("devdock/config/services")
        .join(service_name)
        .join("devdock_config.json");
    
    if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        let json: serde_json::Value = serde_json::from_str(&content)?;
        
        if let Some(service_type) = json.get("type").and_then(|v| v.as_str()) {
            return Ok(service_type.to_string());
        }
    }
    
    Ok("unknown".to_string())
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
) -> Result<()> {
    // Set up signal handler for graceful shutdown
    let instance_id_clone = instance_id.to_string();
    let state_manager_clone = StateManager::new().await?;

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        output::info("Shutting down DevDock...");
        state_manager_clone
            .update_status(&instance_id_clone, InstanceStatus::Stopping)
            .await
            .ok();
    });

    // Wait for tilt to exit
    let status = tilt_process.wait()?;

    // Update status
    state_manager
        .update_status(instance_id, InstanceStatus::Stopped)
        .await?;

    if status.success() {
        output::success("DevDock stopped");
    } else {
        output::warn(&format!("Tilt exited with status: {}", status));
    }

    Ok(())
}
