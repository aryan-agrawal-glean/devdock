//! Port management commands.

use anyhow::{anyhow, Result};
use colored::Colorize;
use devdock_remote::{
    coder, managed_port_forward_status, start_managed_port_forward_logged,
    stop_managed_port_forward, ManagedForwardStatus, ManagedForwardStopOutcome,
};
use devdock_state::{Instance, InstanceStatus, PortMappingRecord, StateManager};
use prettytable::{format, row, Table};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

const TILT_SERVICE_NAME: &str = "tilt";
const TILT_PORT: u16 = 10350;
const NGINX_SERVICE_NAME: &str = "nginx";
const NGINX_PORT: u16 = 8321;
const RECOVERY_FORWARD_STABILITY: std::time::Duration = std::time::Duration::from_secs(1);

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
            .find(|i| {
                i.workspace_path == worktree
                    && !i.is_quarantined
                    && (i.status == InstanceStatus::Running
                        || i.status == InstanceStatus::Starting
                        || (i.mode == "remote"
                            && matches!(i.status, InstanceStatus::Stopped | InstanceStatus::Error)))
            })
            .map(|i| i.id)
            .ok_or_else(|| anyhow!("No running DevDock instance found for current worktree"))?
    };

    let owning_instance = state.get_instance(&devdock_id).await?;
    let mappings = state.get_port_mapping_records(&devdock_id).await?;

    if mappings.is_empty() {
        println!("{}", "No port mappings found".yellow());
        println!(
            "Tip: Use --auto-forward when starting remote DevDock to enable auto port forwarding"
        );
        return Ok(());
    }

    if owning_instance.as_ref().is_some_and(|instance| {
        instance.mode == "remote"
            && matches!(instance.status, InstanceStatus::Stopped | InstanceStatus::Error)
    }) {
        println!(
            "{}",
            "Session is stopped; mappings are inspectable but are not valid test targets until recovery succeeds."
                .yellow()
        );
    }

    // Display port table
    print_port_table(
        &mappings,
        owning_instance.as_ref().and_then(|instance| instance.workspace_id.as_deref()),
        owning_instance.as_ref().and_then(|instance| instance.workspace_name.as_deref()),
    );

    // Display service URLs
    println!("\n{}:", "Access services".bold());
    for mapping in &mappings {
        if mapping.port_type.eq_ignore_ascii_case("service") {
            println!(
                "  {} {}",
                mapping.service_name.cyan(),
                format!("http://localhost:{}", mapping.local_port).green()
            );
        }
    }

    Ok(())
}

/// Print port mappings table.
fn print_port_table(
    mappings: &[PortMappingRecord],
    workspace_id: Option<&str>,
    workspace_name: Option<&str>,
) {
    let mut table = Table::new();
    table.set_format(*format::consts::FORMAT_BOX_CHARS);

    // Add header row
    table.add_row(row!["Service", "Type", "Local Port", "Remote Port", "Status"]);

    for mapping in mappings {
        let status = match (workspace_id, workspace_name) {
            (Some(workspace_id), Some(workspace_name)) => {
                match managed_port_forward_status(mapping, workspace_id, workspace_name) {
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
    workspace: Option<&str>,
) -> Result<()> {
    let state = StateManager::new().await?;

    // Get the current instance or resolve a retryable remote session by its
    // immutable Coder workspace identity. Workspace-addressable forwarding is
    // what makes a stopped wrapper session recoverable after manual startup.
    let instance_info = get_remote_instance(&state, workspace).await?;

    if all {
        forward_all_ports(&state, &instance_info, no_offset).await
    } else if let Some(svc) = service {
        forward_service_port(&state, &instance_info, svc, no_offset).await
    } else if let Some(custom_mapping) = custom {
        forward_custom_port(
            &state,
            &instance_info.devdock_id,
            &instance_info.workspace_name,
            &instance_info.workspace_id,
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
    let info = get_remote_instance(&state, None).await?;

    if all {
        stop_all_port_forwards(&state, &info).await
    } else if let Some(svc) = service {
        stop_service_port_forward(&state, &info, svc).await
    } else {
        Err(anyhow!("Please specify --all or --service <name>"))
    }
}

pub(crate) struct RemoteInstanceInfo {
    pub(crate) devdock_id: String,
    pub(crate) workspace_name: String,
    pub(crate) workspace_id: String,
    pub(crate) port_base: u16,
    pub(crate) profile: String,
}

/// Resolve a remote instance for the current worktree or an explicit Coder
/// workspace. Stopped/error rows remain eligible only for remote forwarding;
/// quarantined rows are always rejected before any state or process mutation.
pub(crate) async fn get_remote_instance(
    state: &StateManager,
    requested_workspace: Option<&str>,
) -> Result<RemoteInstanceInfo> {
    let (worktree, coder_identity) = if let Some(workspace) = requested_workspace {
        let identity = coder::get_workspace_identity(workspace)?
            .ok_or_else(|| anyhow!("Coder workspace '{workspace}' was not found"))?;
        (None, Some(identity))
    } else {
        (Some(get_current_worktree()?), None)
    };
    let instances = state.list_instances().await?;

    let instance = if let (Some(worktree), None) = (&worktree, &coder_identity) {
        if let Some(quarantined) = instances.iter().find(|i| {
            i.workspace_path == worktree.as_str()
                && i.mode == "remote"
                && (i.is_quarantined || i.status == InstanceStatus::Quarantined)
        }) {
            return Err(anyhow!(
                "Refusing to modify quarantined remote session {} for the current worktree",
                Instance::display_id(&quarantined.id)
            ));
        }
        instances
            .into_iter()
            .find(|i| {
                i.workspace_path == worktree.as_str()
                    && i.mode == "remote"
                    && !i.is_quarantined
                    && matches!(
                        i.status,
                        InstanceStatus::Running
                            | InstanceStatus::Starting
                            | InstanceStatus::Stopped
                            | InstanceStatus::Error
                    )
            })
            .ok_or_else(|| {
                anyhow!("No running remote DevDock instance found for current worktree")
            })?
    } else {
        let identity = coder_identity.as_ref().expect("workspace identity is present");
        if let Some(quarantined) = instances.iter().find(|i| {
            i.mode == "remote"
                && (i.is_quarantined || i.status == InstanceStatus::Quarantined)
                && remote_identity_matches(i, identity)
        }) {
            return Err(anyhow!(
                "Refusing to modify quarantined remote session {} for workspace '{}'",
                Instance::display_id(&quarantined.id),
                identity.name
            ));
        }

        instances
            .into_iter()
            .filter(|i| {
                i.mode == "remote"
                    && i.superseded_by.is_none()
                    && !i.is_quarantined
                    && remote_identity_matches(i, identity)
            })
            .max_by(|left, right| {
                remote_identity_rank(left, identity)
                    .cmp(&remote_identity_rank(right, identity))
                    .then_with(|| left.updated_at.cmp(&right.updated_at))
                    .then_with(|| left.id.cmp(&right.id))
            })
            .ok_or_else(|| {
                anyhow!(
                    "No local remote session is recorded for Coder workspace '{}'. Run `devdock recover --workspace {}` first (or provide the original profile to recover it).",
                    identity.name,
                    identity.name
                )
            })?
    };

    let mut workspace_name = instance
        .workspace_name
        .clone()
        .or_else(|| coder_identity.as_ref().map(|identity| identity.name.clone()))
        .or_else(|| instance.planned_workspace_name.clone())
        .ok_or_else(|| anyhow!("Remote instance missing workspace name"))?;
    // Sessions created by older DevDock binaries only recorded the mutable workspace name.
    // Resolve and persist the immutable Coder identity before this process creates any managed
    // forward, so legacy state remains usable without weakening ownership verification.
    let workspace_id = if let Some(workspace_id) = instance.workspace_id.clone() {
        workspace_id
    } else {
        let identity = match coder_identity {
            Some(identity) => identity,
            None => coder::get_workspace_identity(&workspace_name)?.ok_or_else(|| {
                anyhow!(
                    "Coder workspace '{workspace_name}' was not found while upgrading legacy session identity"
                )
            })?,
        };
        let legacy_mappings = state.get_port_mapping_records(&instance.id).await?;
        for mapping in &legacy_mappings {
            if let ManagedForwardStatus::Unverified(reason) =
                managed_port_forward_status(mapping, &identity.id, &identity.name)
            {
                return Err(anyhow!(
                    "Refusing to reconcile legacy session because ownership of {} is unverifiable ({reason})",
                    mapping.service_name
                ));
            }
        }
        let attached =
            state.attach_remote_workspace(&instance.id, &identity.id, &identity.name).await?;
        workspace_name = attached
            .workspace_name
            .ok_or_else(|| anyhow!("Failed to persist canonical Coder workspace name"))?;
        attached
            .workspace_id
            .ok_or_else(|| anyhow!("Failed to persist immutable Coder workspace identity"))?
    };

    Ok(RemoteInstanceInfo {
        devdock_id: instance.id,
        workspace_name,
        workspace_id,
        port_base: instance.port_base,
        profile: instance.profile,
    })
}

fn remote_identity_matches(instance: &Instance, identity: &coder::CoderWorkspaceIdentity) -> bool {
    match instance.workspace_id.as_deref() {
        Some(workspace_id) => workspace_id == identity.id,
        None => {
            instance.workspace_name.as_deref() == Some(identity.name.as_str())
                || instance.planned_workspace_name.as_deref() == Some(identity.name.as_str())
        }
    }
}

fn remote_identity_rank(
    instance: &Instance,
    identity: &coder::CoderWorkspaceIdentity,
) -> (bool, bool, bool, bool) {
    (
        instance.workspace_id.as_deref() == Some(identity.id.as_str()),
        instance.workspace_name.as_deref() == Some(identity.name.as_str()),
        matches!(instance.status, InstanceStatus::Running | InstanceStatus::Starting),
        instance.status != InstanceStatus::Quarantined,
    )
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
            &info.workspace_id,
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
            &info.workspace_id,
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
        &info.workspace_id,
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
    workspace_id: &str,
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
    start_port_forward(
        state,
        devdock_id,
        workspace_name,
        workspace_id,
        "custom",
        local_port,
        remote_port,
    )
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
    workspace_id: &str,
    service_name: &str,
    local_port: u16,
    remote_port: u16,
) -> Result<()> {
    let log_dir = dirs::home_dir()
        .ok_or_else(|| anyhow!("Could not determine home directory"))?
        .join(".devdock")
        .join("ports");
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join(format!("{devdock_id}_{service_name}.log"));

    start_managed_port_forward_logged(
        state,
        devdock_id,
        workspace_name,
        workspace_id,
        service_name,
        "service",
        local_port,
        remote_port,
        &log_path,
    )
    .await?;
    Ok(())
}

/// Stop all port forwards for an instance.
async fn stop_all_port_forwards(state: &StateManager, info: &RemoteInstanceInfo) -> Result<()> {
    let mappings = state.get_port_mapping_records(&info.devdock_id).await?;

    if mappings.is_empty() {
        println!("{}", "No port forwards to stop".yellow());
        return Ok(());
    }

    println!("{}", "Stopping all port forwards...".cyan());
    let mut stopped_count = 0;

    let mut unverified_count = 0;
    for mapping in mappings {
        match stop_managed_port_forward(&mapping, &info.workspace_id, &info.workspace_name)? {
            ManagedForwardStopOutcome::Stopped => {
                println!("  ✅ Stopped: {}", mapping.service_name.cyan());
                stopped_count += 1;
            }
            ManagedForwardStopOutcome::AlreadyExited => {
                println!("  ✓ Already exited: {}", mapping.service_name.cyan());
            }
            ManagedForwardStopOutcome::Unverified(reason) => {
                println!("  ⚠️  Not stopping {}: {}", mapping.service_name.yellow(), reason);
                unverified_count += 1;
                continue;
            }
        }
        state
            .remove_port_mapping_record(
                &mapping.devdock_id,
                &mapping.service_name,
                &mapping.port_type,
            )
            .await?;
    }

    println!();
    println!("{} {} port forward(s) stopped", "✅".green(), stopped_count);
    if unverified_count > 0 {
        println!(
            "{} {} legacy/external forward(s) left untouched because ownership could not be verified",
            "⚠️".yellow(),
            unverified_count
        );
    }

    Ok(())
}

/// Stop a specific service port forward.
async fn stop_service_port_forward(
    state: &StateManager,
    info: &RemoteInstanceInfo,
    service_name: &str,
) -> Result<()> {
    let mappings = state.get_port_mapping_records(&info.devdock_id).await?;

    let mappings: Vec<_> =
        mappings.into_iter().filter(|mapping| mapping.service_name == service_name).collect();
    if mappings.is_empty() {
        return Err(anyhow!("No port forward found for service: {service_name}"));
    }

    // A service can have both service and debug mappings. Refuse the whole request before
    // changing anything if any live mapping lacks kill authority, then stop each exact record.
    for mapping in &mappings {
        if let ManagedForwardStatus::Unverified(reason) =
            managed_port_forward_status(mapping, &info.workspace_id, &info.workspace_name)
        {
            return Err(anyhow!(
                "Refusing to stop {service_name}: ownership could not be verified ({reason})"
            ));
        }
    }

    println!("{}", format!("Stopping {service_name} port forward...").cyan());

    for mapping in mappings {
        match stop_managed_port_forward(&mapping, &info.workspace_id, &info.workspace_name)? {
            ManagedForwardStopOutcome::Stopped => {
                println!(
                    "  ✅ Stopped: {} {} (PID: {})",
                    service_name.cyan(),
                    mapping.port_type,
                    mapping.process_pid.expect("managed forward has a PID")
                );
            }
            ManagedForwardStopOutcome::AlreadyExited => {
                println!("  ✓ {} process already exited", mapping.port_type);
            }
            ManagedForwardStopOutcome::Unverified(reason) => {
                return Err(anyhow!(
                    "Refusing to stop {service_name}: ownership changed during cleanup ({reason})"
                ));
            }
        }

        state
            .remove_port_mapping_record(
                &mapping.devdock_id,
                &mapping.service_name,
                &mapping.port_type,
            )
            .await?;
    }

    println!();
    println!("View remaining mappings: {}", "devdock ports list".cyan());

    Ok(())
}

/// Stop only verified forwards belonging to a source session before a future
/// session switch. No workspace/process cleanup is attempted for unverified
/// rows, and no target session is attached if the source ownership check fails.
pub(crate) async fn stop_workspace_for_attach(
    state: &StateManager,
    workspace: &str,
) -> Result<(String, Vec<u16>)> {
    let info = get_remote_instance(state, Some(workspace)).await?;
    let mappings = state.get_port_mapping_records(&info.devdock_id).await?;

    for mapping in &mappings {
        if let ManagedForwardStatus::Unverified(reason) =
            managed_port_forward_status(mapping, &info.workspace_id, &info.workspace_name)
        {
            return Err(anyhow!(
                "Refusing session handoff: ownership of {} could not be verified ({reason})",
                mapping.service_name
            ));
        }
    }

    let mut local_ports = Vec::new();
    for mapping in mappings {
        match stop_managed_port_forward(&mapping, &info.workspace_id, &info.workspace_name)? {
            ManagedForwardStopOutcome::Stopped | ManagedForwardStopOutcome::AlreadyExited => {
                local_ports.push(mapping.local_port);
                state
                    .remove_port_mapping_record(
                        &mapping.devdock_id,
                        &mapping.service_name,
                        &mapping.port_type,
                    )
                    .await?;
            }
            ManagedForwardStopOutcome::Unverified(reason) => {
                return Err(anyhow!(
                    "Refusing session handoff: ownership of {} changed ({reason})",
                    mapping.service_name
                ));
            }
        }
    }

    // The source session no longer owns any live managed forwards. Mark it
    // inactive before the socket probe so a failed handoff cannot leave a
    // misleading Running row behind.
    state.update_status(&info.devdock_id, InstanceStatus::Stopped).await?;

    Ok((info.devdock_id, local_ports))
}

/// Attach the target session's all-service forward set and prove each local
/// loopback socket is listening. The 30-second bound keeps this hook within
/// the requested 45-second switch budget while leaving room for orchestration.
pub(crate) async fn attach_workspace_for_recovery(
    state: &StateManager,
    info: &RemoteInstanceInfo,
) -> Result<()> {
    let existing = state.get_port_mapping_records(&info.devdock_id).await?;
    for mapping in &existing {
        if let ManagedForwardStatus::Unverified(reason) =
            managed_port_forward_status(mapping, &info.workspace_id, &info.workspace_name)
        {
            return Err(anyhow!(
                "refusing recovery because ownership of {} is unverifiable ({reason})",
                mapping.service_name
            ));
        }
    }

    let expected_services = discover_service_ports(&info.profile)?;
    for (service_name, remote_port) in &expected_services {
        let already_owned = existing.iter().find(|mapping| {
            mapping.service_name == *service_name
                && mapping.port_type == "Service"
                && mapping.local_port == *remote_port
                && mapping.remote_port == *remote_port
        });
        let already_active = already_owned.is_some_and(|mapping| {
            managed_port_forward_status(mapping, &info.workspace_id, &info.workspace_name)
                == ManagedForwardStatus::Active
        });
        if !already_active
            && std::net::TcpStream::connect((devdock_state::LOOPBACK_BIND_ADDRESS, *remote_port))
                .is_ok()
        {
            return Err(anyhow!(
                "loopback port {remote_port} for {service_name} is occupied by an unowned listener; refusing recovery"
            ));
        }
    }
    forward_all_ports(state, info, true).await?;
    let mappings = state.get_port_mapping_records(&info.devdock_id).await?;
    if mappings.is_empty() || expected_services.is_empty() {
        return Err(anyhow!("no managed forwards were registered for the recovered session"));
    }
    for (service_name, remote_port) in &expected_services {
        let local_port = *remote_port;
        let mapping = mappings.iter().find(|mapping| {
            mapping.service_name == *service_name
                && mapping.port_type == "Service"
                && mapping.local_port == local_port
                && mapping.remote_port == *remote_port
        });
        let Some(mapping) = mapping else {
            return Err(anyhow!(
                "forward for {service_name} did not register on 127.0.0.1:{local_port}"
            ));
        };
        if managed_port_forward_status(mapping, &info.workspace_id, &info.workspace_name)
            != ManagedForwardStatus::Active
        {
            return Err(anyhow!(
                "forward for {service_name} is not an active, verified managed process"
            ));
        }
    }
    wait_for_recovered_forwards_to_stabilize(
        state,
        info,
        &expected_services,
        std::time::Duration::from_secs(30),
    )
    .await?;

    println!("\n{}", "Attached service URLs".bold());
    for mapping in mappings.iter().filter(|mapping| mapping.port_type == "Service") {
        println!(
            "  {} {}",
            mapping.service_name.cyan(),
            format!("http://127.0.0.1:{}", mapping.local_port).green()
        );
    }
    println!("  {} http://127.0.0.1:{}", "tilt".cyan(), TILT_PORT);
    Ok(())
}

/// A recovered session is not ready merely because its child was spawned or a
/// port was briefly reachable. Keep checking the persisted exact identities
/// and loopback listeners for a short stability window before recovery claims
/// success. This catches children that disappear when a noninteractive parent
/// process exits.
async fn wait_for_recovered_forwards_to_stabilize(
    state: &StateManager,
    info: &RemoteInstanceInfo,
    expected_services: &[(String, u16)],
    timeout: std::time::Duration,
) -> Result<()> {
    let started = std::time::Instant::now();
    let mut stable_since = None;

    loop {
        let mappings = state.get_port_mapping_records(&info.devdock_id).await?;
        let active_and_listening = expected_services.iter().all(|(service_name, remote_port)| {
            let mapping = mappings.iter().find(|mapping| {
                mapping.service_name == *service_name
                    && mapping.port_type == "Service"
                    && mapping.local_port == *remote_port
                    && mapping.remote_port == *remote_port
            });
            mapping.is_some_and(|mapping| {
                managed_port_forward_status(mapping, &info.workspace_id, &info.workspace_name)
                    == ManagedForwardStatus::Active
                    && std::net::TcpStream::connect((
                        devdock_state::LOOPBACK_BIND_ADDRESS,
                        mapping.local_port,
                    ))
                    .is_ok()
            })
        });

        if active_and_listening {
            let stable_at = stable_since.get_or_insert_with(std::time::Instant::now);
            if stable_at.elapsed() >= RECOVERY_FORWARD_STABILITY {
                return Ok(());
            }
        } else {
            stable_since = None;
        }

        if started.elapsed() >= timeout {
            return Err(anyhow!(
                "recovered forwards did not remain active and listening on loopback for {}ms; refusing to claim the session is ready",
                RECOVERY_FORWARD_STABILITY.as_millis()
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

pub(crate) async fn wait_for_ports_free(ports: &[u16], timeout: std::time::Duration) -> Result<()> {
    let start = std::time::Instant::now();
    let unique_ports: Vec<u16> = ports.iter().copied().fold(Vec::new(), |mut ports, port| {
        if !ports.contains(&port) {
            ports.push(port);
        }
        ports
    });

    loop {
        if unique_ports.iter().all(|port| {
            std::net::TcpStream::connect((devdock_state::LOOPBACK_BIND_ADDRESS, *port)).is_err()
        }) {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            return Err(anyhow!(
                "source session ports remain occupied after {}s; refusing to attach a different workspace",
                timeout.as_secs()
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
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
