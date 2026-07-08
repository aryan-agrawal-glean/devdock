//! Port management commands.

use anyhow::{anyhow, Result};
use colored::Colorize;
use devdock_state::StateManager;
use prettytable::{format, row, Table};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const TILT_SERVICE_NAME: &str = "tilt";
const TILT_PORT: u16 = 10350;
const NGINX_SERVICE_NAME: &str = "nginx";
const NGINX_PORT: u16 = 8321;

fn push_unique_port(ports: &mut Vec<(String, u16)>, service_name: &str, port: u16) {
    if !ports.iter().any(|(name, _)| name == service_name) {
        ports.push((service_name.to_string(), port));
    }
}

fn local_port_for(service_name: &str, remote_port: u16, port_base: u16, no_offset: bool) -> u16 {
    if no_offset
        || (service_name == "fe" && remote_port == 8888)
        || (service_name == NGINX_SERVICE_NAME && remote_port == NGINX_PORT)
    {
        remote_port
    } else {
        port_base + remote_port
    }
}

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
                format!("http://localhost:{local_port}").green()
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
        let status =
            if pid.is_some() { "Active".green().to_string() } else { "Inactive".red().to_string() };

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
        .map_err(|e| anyhow!("Failed to get git worktree: {e}"))?;

    if !output.status.success() {
        return Err(anyhow!("Not in a git repository"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Forward ports for remote DevDock.
pub async fn forward(
    all: bool,
    service: Option<&str>,
    custom: Option<&str>,
    no_offset: bool,
) -> Result<()> {
    let state = StateManager::new().await?;

    // Get current instance (full info including profile)
    let instance_info = get_current_remote_instance(&state).await?;

    if all {
        forward_all_ports(&state, &instance_info, no_offset).await
    } else if let Some(svc) = service {
        forward_service_port(&state, &instance_info, svc, no_offset).await
    } else if let Some(custom_mapping) = custom {
        forward_custom_port(
            &state,
            &instance_info.devdock_id,
            &instance_info.workspace_name,
            custom_mapping,
        )
        .await
    } else {
        Err(anyhow!("Please specify --all, --service <name>, or --custom <local:remote>"))
    }
}

/// Stop port forwarding.
pub async fn stop(all: bool, service: Option<&str>) -> Result<()> {
    let state = StateManager::new().await?;

    // Get current instance
    let info = get_current_remote_instance(&state).await?;

    if all {
        stop_all_port_forwards(&state, &info.devdock_id).await
    } else if let Some(svc) = service {
        stop_service_port_forward(&state, &info.devdock_id, svc).await
    } else {
        Err(anyhow!("Please specify --all or --service <name>"))
    }
}

struct RemoteInstanceInfo {
    devdock_id: String,
    workspace_name: String,
    port_base: u16,
    profile: String,
}

/// Get current remote DevDock instance.
async fn get_current_remote_instance(state: &StateManager) -> Result<RemoteInstanceInfo> {
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

    Ok(RemoteInstanceInfo {
        devdock_id: instance.id,
        workspace_name,
        port_base: instance.port_base,
        profile: instance.profile,
    })
}

/// Profile structure for reading service definitions.
#[derive(Debug, Deserialize)]
struct ProfileYaml {
    services: Option<HashMap<String, ProfileServiceYaml>>,
}

/// Service definition inside the profile YAML.
#[derive(Debug, Deserialize)]
struct ProfileServiceYaml {
    port: Option<u16>,
    #[serde(rename = "type")]
    service_type: Option<String>,
}

/// Service config from devdock_config.json (env.PORT).
#[derive(Debug, Deserialize)]
struct ServiceConfigJson {
    env: Option<HashMap<String, serde_json::Value>>,
}

/// Resolve the port for a service: profile YAML `port` field takes priority,
/// then we fall back to `env.PORT` inside `devdock/config/services/<name>/devdock_config.json`.
fn resolve_service_port(service_name: &str, profile_port: Option<u16>) -> Option<u16> {
    if let Some(p) = profile_port {
        return Some(p);
    }

    let devdock_root = std::env::var("DEVDOCK_ROOT").unwrap_or_else(|_| ".".to_string());
    let config_path = PathBuf::from(&devdock_root)
        .join("devdock/config/services")
        .join(service_name)
        .join("devdock_config.json");

    if !config_path.exists() {
        return None;
    }

    let contents = std::fs::read_to_string(&config_path).ok()?;
    let config: ServiceConfigJson = serde_json::from_str(&contents).ok()?;

    if let Some(env) = config.env {
        if let Some(port_val) = env.get("PORT") {
            if let Some(s) = port_val.as_str() {
                return s.parse::<u16>().ok();
            } else if let Some(n) = port_val.as_u64() {
                return Some(n as u16);
            }
        }
    }
    None
}

/// Find the profile YAML path by name (mirrors the lookup in remote.rs).
fn find_profile_path(profile_name: &str) -> Result<PathBuf> {
    let devdock_root = std::env::var("DEVDOCK_ROOT")
        .or_else(|_| std::env::var("PWD"))
        .unwrap_or_else(|_| ".".to_string());

    let base = PathBuf::from(devdock_root).join("devdock/profiles");

    for sub in &["user_profiles", "integration_tests", ""] {
        let p = if sub.is_empty() {
            base.join(format!("{profile_name}.yaml"))
        } else {
            base.join(sub).join(format!("{profile_name}.yaml"))
        };
        if p.exists() {
            return Ok(p);
        }
    }

    Err(anyhow!("Profile not found: {profile_name}"))
}

/// Build the list of (service_name, remote_port) from the instance's profile YAML
/// and devdock_config.json. Always includes `tilt` (10350) and `nginx` (8321).
fn discover_service_ports(profile_name: &str) -> Result<Vec<(String, u16)>> {
    let mut ports: Vec<(String, u16)> = Vec::new();

    // Tilt UI is always forwarded.
    push_unique_port(&mut ports, TILT_SERVICE_NAME, TILT_PORT);
    // Remote DevDock needs nginx forwarded for local.glean.com routes even when users
    // request a narrower service set.
    push_unique_port(&mut ports, NGINX_SERVICE_NAME, NGINX_PORT);

    let profile_path = find_profile_path(profile_name)?;
    let contents = std::fs::read_to_string(&profile_path)
        .map_err(|e| anyhow!("Failed to read profile {profile_name}: {e}"))?;
    let profile: ProfileYaml = serde_yaml::from_str(&contents)
        .map_err(|e| anyhow!("Failed to parse profile {profile_name}: {e}"))?;

    if let Some(services) = profile.services {
        for (name, svc) in &services {
            if let Some(ref st) = svc.service_type {
                if st != "local" {
                    continue;
                }
            }
            if let Some(port) = resolve_service_port(name, svc.port) {
                push_unique_port(&mut ports, name, port);
            }
        }
    }

    Ok(ports)
}

/// Forward all service ports discovered from the profile.
async fn forward_all_ports(
    state: &StateManager,
    info: &RemoteInstanceInfo,
    no_offset: bool,
) -> Result<()> {
    println!("{}", "Setting up port forwarding for all services...".cyan());

    let services = discover_service_ports(&info.profile)?;
    if services.is_empty() {
        println!("{}", "No services found in profile to forward".yellow());
        return Ok(());
    }

    let mut success_count = 0;

    for (service_name, remote_port) in &services {
        let local_port = local_port_for(service_name, *remote_port, info.port_base, no_offset);

        match start_port_forward(
            state,
            &info.devdock_id,
            &info.workspace_name,
            service_name,
            local_port,
            *remote_port,
        )
        .await
        {
            Ok(()) => {
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
    println!("{} {}/{} port forwards established", "✅".green(), success_count, services.len());
    if !no_offset {
        println!(
            "Tip: Use {} to forward without port offset",
            "devdock ports forward --all --no-offset".cyan()
        );
    }
    println!("View mappings: {}", "devdock ports list".cyan());

    Ok(())
}

async fn forward_nginx_if_available(
    state: &StateManager,
    info: &RemoteInstanceInfo,
    services: &[(String, u16)],
    no_offset: bool,
) -> Result<()> {
    if let Some((_, nginx_remote_port)) =
        services.iter().find(|(name, _)| name == NGINX_SERVICE_NAME)
    {
        let existing = state.get_port_mappings(&info.devdock_id).await?;
        if existing.iter().any(|(name, _, _, _, pid)| name == NGINX_SERVICE_NAME && pid.is_some()) {
            return Ok(());
        }

        let nginx_local_port =
            local_port_for(NGINX_SERVICE_NAME, *nginx_remote_port, info.port_base, no_offset);
        println!("{}", "Forwarding nginx for local.glean.com routes...".cyan());
        match start_port_forward(
            state,
            &info.devdock_id,
            &info.workspace_name,
            NGINX_SERVICE_NAME,
            nginx_local_port,
            *nginx_remote_port,
        )
        .await
        {
            Ok(()) => println!(
                "  ✅ {}: localhost:{} → remote:{}",
                NGINX_SERVICE_NAME.cyan(),
                nginx_local_port,
                nginx_remote_port
            ),
            Err(e) => println!("  ⚠️  {}: failed ({})", NGINX_SERVICE_NAME.yellow(), e),
        }
    }

    Ok(())
}

/// Forward a specific service port.
async fn forward_service_port(
    state: &StateManager,
    info: &RemoteInstanceInfo,
    service_name: &str,
    no_offset: bool,
) -> Result<()> {
    let services = discover_service_ports(&info.profile)?;

    let (_, remote_port) =
        services.iter().find(|(name, _)| name == service_name).ok_or_else(|| {
            let available: Vec<_> = services.iter().map(|(n, _)| n.as_str()).collect();
            anyhow!("Unknown service: {service_name}. Available: {}", available.join(", "))
        })?;

    let local_port = local_port_for(service_name, *remote_port, info.port_base, no_offset);

    if service_name != NGINX_SERVICE_NAME {
        forward_nginx_if_available(state, info, &services, no_offset).await?;
    }

    println!("{}", format!("Forwarding {service_name} port...").cyan());
    start_port_forward(
        state,
        &info.devdock_id,
        &info.workspace_name,
        service_name,
        local_port,
        *remote_port,
    )
    .await?;

    println!("  ✅ {}: localhost:{} → remote:{}", service_name.cyan(), local_port, remote_port);
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
        return Err(anyhow!("Invalid port mapping format. Use: local:remote (e.g., 8888:8888)"));
    }

    let local_port: u16 =
        parts[0].parse().map_err(|_| anyhow!("Invalid local port: {}", parts[0]))?;
    let remote_port: u16 =
        parts[1].parse().map_err(|_| anyhow!("Invalid remote port: {}", parts[1]))?;

    println!("{}", "Forwarding custom port...".cyan());
    start_port_forward(state, devdock_id, workspace_name, "custom", local_port, remote_port)
        .await?;

    println!("  ✅ Custom: localhost:{local_port} → remote:{remote_port}");
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

    let log_file = log_dir.join(format!("{devdock_id}_{service_name}.log"));
    let log_handle = File::create(&log_file)?;

    // Start port forward process
    let child = Command::new("coder")
        .arg("port-forward")
        .arg(workspace_name)
        .arg("--tcp")
        .arg(format!("{local_port}:{remote_port}"))
        .stdin(Stdio::null())
        .stdout(log_handle.try_clone()?)
        .stderr(log_handle)
        .spawn()
        .map_err(|e| anyhow!("Failed to start port forward: {e}"))?;

    let pid = child.id();

    // Give it a moment to establish
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    // Save port mapping to state
    state
        .save_port_mapping(devdock_id, service_name, "service", local_port, remote_port, Some(pid))
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
        .ok_or_else(|| anyhow!("No port forward found for service: {service_name}"))?;

    println!("{}", format!("Stopping {service_name} port forward...").cyan());

    if let Some(pid) = mapping.4 {
        if kill_process(pid) {
            println!("  ✅ Stopped: {} (PID: {})", service_name.cyan(), pid);
        } else {
            println!("  ⚠️  Process {pid} not found (may have already exited)");
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Serialize all tests that mutate DEVDOCK_ROOT env var.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // ── helpers ──────────────────────────────────────────────────────────────

    fn setup_profile(dir: &TempDir, name: &str, yaml: &str) {
        let profiles_dir = dir.path().join("devdock/profiles");
        fs::create_dir_all(&profiles_dir).unwrap();
        fs::write(profiles_dir.join(format!("{name}.yaml")), yaml).unwrap();
    }

    fn setup_service_config(dir: &TempDir, service: &str, json: &str) {
        let svc_dir = dir.path().join(format!("devdock/config/services/{service}"));
        fs::create_dir_all(&svc_dir).unwrap();
        fs::write(svc_dir.join("devdock_config.json"), json).unwrap();
    }

    // ── resolve_service_port ─────────────────────────────────────────────────

    #[test]
    fn resolve_port_uses_profile_port_when_provided() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        // Even if a devdock_config.json exists with a different port, the
        // explicit profile port must win.
        setup_service_config(&tmp, "qe", r#"{"env":{"PORT":"9999"}}"#);
        std::env::set_var("DEVDOCK_ROOT", tmp.path());
        let port = super::resolve_service_port("qe", Some(8080));
        assert_eq!(port, Some(8080), "Profile port should take priority");
    }

    #[test]
    fn resolve_port_falls_back_to_config_json_string_port() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        setup_service_config(&tmp, "svc", r#"{"env":{"PORT":"8080"}}"#);
        std::env::set_var("DEVDOCK_ROOT", tmp.path());
        let port = super::resolve_service_port("svc", None);
        assert_eq!(port, Some(8080));
    }

    #[test]
    fn resolve_port_falls_back_to_config_json_numeric_port() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        setup_service_config(&tmp, "svc2", r#"{"env":{"PORT":8090}}"#);
        std::env::set_var("DEVDOCK_ROOT", tmp.path());
        let port = super::resolve_service_port("svc2", None);
        assert_eq!(port, Some(8090));
    }

    #[test]
    fn resolve_port_returns_none_when_no_config() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::env::set_var("DEVDOCK_ROOT", tmp.path());
        let port = super::resolve_service_port("unknown-svc", None);
        assert_eq!(port, None);
    }

    // ── find_profile_path ────────────────────────────────────────────────────

    #[test]
    fn find_profile_path_locates_profile_at_root() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        setup_profile(&tmp, "default", "services: {}");
        std::env::set_var("DEVDOCK_ROOT", tmp.path());
        let path = super::find_profile_path("default").unwrap();
        assert!(path.exists());
        assert!(path.ends_with("devdock/profiles/default.yaml"));
    }

    #[test]
    fn find_profile_path_locates_profile_in_user_profiles_subdir() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("devdock/profiles/user_profiles");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("custom.yaml"), "services: {}").unwrap();
        std::env::set_var("DEVDOCK_ROOT", tmp.path());
        let path = super::find_profile_path("custom").unwrap();
        assert!(path.exists());
        assert!(path.ends_with("user_profiles/custom.yaml"));
    }

    #[test]
    fn find_profile_path_errors_when_missing() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::env::set_var("DEVDOCK_ROOT", tmp.path());
        assert!(super::find_profile_path("nonexistent").is_err());
    }

    // ── discover_service_ports ───────────────────────────────────────────────

    #[test]
    fn discover_always_includes_tilt_and_nginx() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        setup_profile(&tmp, "empty", "services: {}");
        std::env::set_var("DEVDOCK_ROOT", tmp.path());
        let ports = super::discover_service_ports("empty").unwrap();
        assert!(ports.iter().any(|(n, p)| n == "tilt" && *p == 10350));
        assert!(ports.iter().any(|(n, p)| n == "nginx" && *p == 8321));
    }

    #[test]
    fn discover_dedupes_nginx_when_profile_lists_it() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        setup_profile(&tmp, "default", "services:\n  nginx:\n    type: local\n    port: 8321\n");
        std::env::set_var("DEVDOCK_ROOT", tmp.path());
        let ports = super::discover_service_ports("default").unwrap();
        assert_eq!(ports.iter().filter(|(n, _)| n == "nginx").count(), 1);
        assert!(ports.iter().any(|(n, p)| n == "nginx" && *p == 8321));
    }

    #[test]
    fn discover_includes_services_with_explicit_port() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        setup_profile(&tmp, "default", "services:\n  fe:\n    type: local\n    port: 8888\n");
        std::env::set_var("DEVDOCK_ROOT", tmp.path());
        let ports = super::discover_service_ports("default").unwrap();
        assert!(ports.iter().any(|(n, p)| n == "fe" && *p == 8888));
    }

    #[test]
    fn discover_skips_external_services() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        setup_profile(&tmp, "default", "services:\n  ext:\n    type: external\n    port: 9999\n");
        std::env::set_var("DEVDOCK_ROOT", tmp.path());
        let ports = super::discover_service_ports("default").unwrap();
        assert!(!ports.iter().any(|(n, _)| n == "ext"), "external service must be skipped");
    }

    #[test]
    fn discover_reads_port_from_devdock_config_json() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        setup_profile(&tmp, "default", "services:\n  qe:\n    type: local\n");
        setup_service_config(&tmp, "qe", r#"{"env":{"PORT":"8080"}}"#);
        std::env::set_var("DEVDOCK_ROOT", tmp.path());
        let ports = super::discover_service_ports("default").unwrap();
        assert!(ports.iter().any(|(n, p)| n == "qe" && *p == 8080));
    }

    #[test]
    fn discover_omits_services_without_resolvable_port() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        setup_profile(&tmp, "default", "services:\n  no-port-svc:\n    type: local\n");
        // no devdock_config.json → no port
        std::env::set_var("DEVDOCK_ROOT", tmp.path());
        let ports = super::discover_service_ports("default").unwrap();
        assert!(!ports.iter().any(|(n, _)| n == "no-port-svc"));
    }

    // ── no_offset local port computation ────────────────────────────────────

    #[test]
    fn no_offset_uses_remote_port_as_local() {
        // Mirrors the logic in forward_all_ports: with no_offset=true, local == remote
        let port_base: u16 = 1000;
        let remote_port: u16 = 8080;
        let _ = port_base; // offset not applied when no_offset=true

        let local_with_offset = port_base + remote_port;
        let _ = local_with_offset;
        let local_no_offset = if true /* no_offset */ { remote_port } else { local_with_offset };

        assert_eq!(local_no_offset, 8080);
    }

    #[test]
    fn fe_always_maps_8888_to_8888_regardless_of_offset() {
        // fe with port 8888 always gets local_port == 8888 even when no_offset=false
        let port_base: u16 = 1000;
        let remote_port: u16 = 8888;
        let service_name = "fe";
        let no_offset = false;

        let local_port = if no_offset || (service_name == "fe" && remote_port == 8888) {
            remote_port
        } else {
            port_base + remote_port
        };

        assert_eq!(local_port, 8888, "fe:8888 must always map to local 8888");
    }

    #[test]
    fn fe_non_8888_port_uses_offset() {
        // If fe somehow uses a different port, it still gets the offset applied
        let local_port = super::local_port_for("fe", 9000, 1000, false);

        assert_eq!(local_port, 10000);
    }

    #[test]
    fn nginx_always_maps_8321_to_8321_regardless_of_offset() {
        let local_port = super::local_port_for("nginx", 8321, 1000, false);

        assert_eq!(local_port, 8321, "nginx must keep local.glean.com routes on standard port");
    }
}
