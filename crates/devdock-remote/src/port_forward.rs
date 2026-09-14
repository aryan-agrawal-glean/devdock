//! Port forwarding management for remote workspaces.

use crate::{coder, RemoteError, Result};
use devdock_config::resolve_profile;
use devdock_core::profile::Profile as CoreProfile;
use devdock_state::{
    PortMappingRecord, SessionManifestV1, StateManager, DEVDOCK_FORWARD_OWNER,
    LOOPBACK_BIND_ADDRESS,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::Duration;
use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, Signal, System, UpdateKind};
use tracing::{debug, info, warn};

const TILT_SERVICE_NAME: &str = "tilt";
const TILT_PORT: u16 = 10350;
const NGINX_SERVICE_NAME: &str = "nginx";
const NGINX_PORT: u16 = 8321;

fn push_unique_mapping(
    mappings: &mut Vec<PortMapping>,
    service_name: &str,
    port_type: PortType,
    local_port: u16,
    remote_port: u16,
) {
    if !mappings
        .iter()
        .any(|mapping| mapping.service_name == service_name && mapping.port_type == port_type)
    {
        mappings.push(PortMapping {
            service_name: service_name.to_string(),
            port_type,
            bind_address: LOOPBACK_BIND_ADDRESS.to_string(),
            local_port,
            remote_port,
            process_pid: None,
        });
    }
}

fn local_port_for(service_name: &str, remote_port: u16, offset: u16) -> u16 {
    if service_name == NGINX_SERVICE_NAME && remote_port == NGINX_PORT {
        remote_port
    } else {
        offset + remote_port
    }
}

/// Type of port being forwarded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortType {
    /// Main service port
    Service,
    /// Debug port
    Debug,
    /// Tilt UI port
    TiltUI,
    /// Other named port
    Other(String),
}

impl PortType {
    fn state_name(&self) -> String {
        match self {
            // Preserve the established on-disk strings used by older binaries.
            Self::Service => "Service".to_string(),
            Self::Debug => "Debug".to_string(),
            Self::TiltUI => "TiltUI".to_string(),
            Self::Other(name) => name.clone(),
        }
    }

    fn from_state_name(name: &str) -> Self {
        let normalized = name.to_ascii_lowercase().replace('_', "");
        match normalized.as_str() {
            "service" => Self::Service,
            "debug" => Self::Debug,
            "tiltui" => Self::TiltUI,
            _ => Self::Other(name.to_string()),
        }
    }
}

fn canonical_port_type_name(name: &str) -> String {
    PortType::from_state_name(name).state_name()
}

fn port_type_names_match(left: &str, right: &str) -> bool {
    PortType::from_state_name(left) == PortType::from_state_name(right)
}

/// Represents a port mapping from remote to local with offset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    pub service_name: String,
    pub port_type: PortType,
    pub bind_address: String,
    pub local_port: u16,
    pub remote_port: u16,
    pub process_pid: Option<u32>,
}

/// Manages port forwarding for a remote workspace.
pub struct PortForwardManager {
    workspace_name: String,
    workspace_id: String,
    devdock_id: String,
    #[allow(dead_code)]
    offset: u16,
    best_effort_spawn: bool,
    pub mappings: Vec<PortMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedForwardStopOutcome {
    Stopped,
    AlreadyExited,
    Unverified(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedForwardStatus {
    Active,
    Exited,
    Unverified(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessIdentity {
    start_time: u64,
    command_fingerprint: String,
}

struct VerifiedLiveProcess {
    system: System,
    pid: Pid,
}

/// Profile service definition (simplified).
#[derive(Debug, Deserialize)]
struct ProfileService {
    port: Option<u16>,
    debug_port: Option<u16>,
    #[serde(rename = "type")]
    service_type: Option<String>,
}

/// Profile structure (simplified).
#[derive(Debug, Deserialize)]
struct Profile {
    #[serde(default)]
    project: String,
    services: Option<HashMap<String, ProfileService>>,
}

/// Service config from devdock_config.json
#[derive(Debug, Deserialize)]
struct ServiceConfig {
    env: Option<HashMap<String, serde_json::Value>>,
    debug_port: Option<serde_json::Value>,
}

/// Read service port from devdock_config.json
fn read_service_config_port(service_name: &str) -> Option<u16> {
    let config_path =
        PathBuf::from(std::env::var("DEVDOCK_ROOT").unwrap_or_else(|_| ".".to_string()))
            .join("devdock/config/services")
            .join(service_name)
            .join("devdock_config.json");

    if !config_path.exists() {
        debug!("No config found for service: {} at {:?}", service_name, config_path);
        return None;
    }

    match fs::read_to_string(&config_path) {
        Ok(contents) => {
            if let Ok(config) = serde_json::from_str::<ServiceConfig>(&contents) {
                // Try to get PORT from env
                if let Some(env) = config.env {
                    if let Some(port_value) = env.get("PORT") {
                        // PORT can be a string or number
                        if let Some(port_str) = port_value.as_str() {
                            if let Ok(port) = port_str.parse::<u16>() {
                                debug!(
                                    "Found port {} for service {} from config",
                                    port, service_name
                                );
                                return Some(port);
                            }
                        } else if let Some(port_num) = port_value.as_u64() {
                            debug!(
                                "Found port {} for service {} from config",
                                port_num, service_name
                            );
                            return Some(port_num as u16);
                        }
                    }
                }
            }
        }
        Err(e) => {
            debug!("Failed to read config for {}: {}", service_name, e);
        }
    }

    None
}

/// Read service debug port from devdock_config.json
fn read_service_config_debug_port(service_name: &str) -> Option<u16> {
    let config_path =
        PathBuf::from(std::env::var("DEVDOCK_ROOT").unwrap_or_else(|_| ".".to_string()))
            .join("devdock/config/services")
            .join(service_name)
            .join("devdock_config.json");

    if !config_path.exists() {
        return None;
    }

    match fs::read_to_string(&config_path) {
        Ok(contents) => {
            if let Ok(config) = serde_json::from_str::<ServiceConfig>(&contents) {
                if let Some(debug_port_value) = config.debug_port {
                    // debug_port can be a string or number
                    if let Some(port_str) = debug_port_value.as_str() {
                        if let Ok(port) = port_str.parse::<u16>() {
                            debug!(
                                "Found debug port {} for service {} from config",
                                port, service_name
                            );
                            return Some(port);
                        }
                    } else if let Some(port_num) = debug_port_value.as_u64() {
                        debug!(
                            "Found debug port {} for service {} from config",
                            port_num, service_name
                        );
                        return Some(port_num as u16);
                    }
                }
            }
        }
        Err(e) => {
            debug!("Failed to read config for {}: {}", service_name, e);
        }
    }

    None
}

impl PortForwardManager {
    /// Create port forward manager from profile YAML.
    pub fn from_profile(
        profile_path: &Path,
        workspace: &str,
        workspace_id: &str,
        devdock_id: &str,
        offset: u16,
    ) -> Result<Self> {
        info!("Parsing profile for port mappings: {:?}", profile_path);

        let contents = fs::read_to_string(profile_path)
            .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to read profile: {e}")))?;

        let mut profile: Profile = serde_yaml::from_str(&contents)
            .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to parse profile: {e}")))?;

        // Preserve this module's intentionally simplified service shape while
        // routing the identity-bearing project through the shared loader boundary.
        let identity_profile: CoreProfile =
            serde_json::from_value(serde_json::json!({"project": profile.project})).map_err(
                |e| RemoteError::ConfigSyncError(format!("Failed to parse profile identity: {e}")),
            )?;
        profile.project = resolve_profile(identity_profile)
            .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to resolve profile: {e}")))?
            .project;

        let mut mappings = Vec::new();

        // Add Tilt UI port (always present).
        push_unique_mapping(
            &mut mappings,
            TILT_SERVICE_NAME,
            PortType::TiltUI,
            offset + TILT_PORT,
            TILT_PORT,
        );
        // Remote DevDock should always forward nginx so local.glean.com routes work.
        push_unique_mapping(
            &mut mappings,
            NGINX_SERVICE_NAME,
            PortType::Service,
            local_port_for(NGINX_SERVICE_NAME, NGINX_PORT, offset),
            NGINX_PORT,
        );

        // Extract service ports
        if let Some(services) = profile.services {
            for (service_name, service) in services {
                // Skip if service type is not local
                if let Some(ref service_type) = service.service_type {
                    if service_type != "local" {
                        debug!("Skipping non-local service: {}", service_name);
                        continue;
                    }
                }

                // Add main service port (priority: explicit > config file)
                let port = service.port.or_else(|| read_service_config_port(&service_name));
                if let Some(port) = port {
                    push_unique_mapping(
                        &mut mappings,
                        &service_name,
                        PortType::Service,
                        local_port_for(&service_name, port, offset),
                        port,
                    );
                    debug!("Added port mapping for {}: {}->{}", service_name, offset + port, port);
                } else {
                    debug!("No port mapping found for service: {}", service_name);
                }

                // Add debug port if present (priority: explicit > config file)
                let debug_port =
                    service.debug_port.or_else(|| read_service_config_debug_port(&service_name));
                if let Some(debug_port) = debug_port {
                    push_unique_mapping(
                        &mut mappings,
                        &service_name,
                        PortType::Debug,
                        offset + debug_port,
                        debug_port,
                    );
                    debug!(
                        "Added debug port mapping for {}: {}->{}",
                        service_name,
                        offset + debug_port,
                        debug_port
                    );
                }
            }
        }

        debug!("Created {} port mappings", mappings.len());

        Ok(Self {
            workspace_name: workspace.to_string(),
            workspace_id: workspace_id.to_string(),
            devdock_id: devdock_id.to_string(),
            offset,
            best_effort_spawn: true,
            mappings,
        })
    }

    /// Render exact, raw Coder commands for the forwards selected by a remote
    /// start request. This is recovery guidance only; it never registers or
    /// adopts an externally started process.
    pub fn manual_forward_commands(
        profile_path: &Path,
        workspace: &str,
        offset: u16,
        services: Option<&str>,
    ) -> Result<Vec<String>> {
        let manager = Self::from_profile(profile_path, workspace, "manual", "manual", offset)?;
        let selected = services.map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect::<std::collections::HashSet<_>>()
        });

        Ok(manager
            .mappings
            .iter()
            .filter(|mapping| {
                selected.as_ref().map_or(true, |names| {
                    names.contains(mapping.service_name.as_str())
                        || mapping.service_name == TILT_SERVICE_NAME
                        || mapping.service_name == NGINX_SERVICE_NAME
                })
            })
            .map(|mapping| {
                coder::port_forward_command_line(workspace, mapping.local_port, mapping.remote_port)
            })
            .collect())
    }

    /// Create the compatibility-mode Tilt and nginx forwards without parsing
    /// the rest of the profile a second time.
    pub fn default_forwards(
        workspace: &str,
        workspace_id: &str,
        devdock_id: &str,
        offset: u16,
    ) -> Self {
        let mut mappings = Vec::new();
        push_unique_mapping(
            &mut mappings,
            TILT_SERVICE_NAME,
            PortType::TiltUI,
            offset + TILT_PORT,
            TILT_PORT,
        );
        push_unique_mapping(
            &mut mappings,
            NGINX_SERVICE_NAME,
            PortType::Service,
            local_port_for(NGINX_SERVICE_NAME, NGINX_PORT, offset),
            NGINX_PORT,
        );
        Self {
            workspace_name: workspace.to_string(),
            workspace_id: workspace_id.to_string(),
            devdock_id: devdock_id.to_string(),
            offset,
            best_effort_spawn: false,
            mappings,
        }
    }

    /// Reconstruct resolved mappings from durable session state. This never
    /// re-reads a mutable or deleted profile.
    pub fn from_manifest(manifest: &SessionManifestV1) -> Result<Self> {
        let workspace_name =
            manifest.workspace_name.as_deref().unwrap_or(&manifest.planned_workspace_name);
        let workspace_id = manifest.workspace_id.as_deref().ok_or_else(|| {
            RemoteError::PortForwardError(
                "session manifest is missing immutable workspace identity".to_string(),
            )
        })?;
        let mut mappings = Vec::with_capacity(manifest.port_mappings.len());
        for mapping in &manifest.port_mappings {
            if mapping.bind_address != LOOPBACK_BIND_ADDRESS {
                return Err(RemoteError::PortForwardError(format!(
                    "managed forward for {} has invalid bind address {}",
                    mapping.service_name, mapping.bind_address
                )));
            }
            if mapping.workspace_id != workspace_id {
                return Err(RemoteError::PortForwardError(format!(
                    "managed forward for {} belongs to a different workspace",
                    mapping.service_name
                )));
            }
            if mapping.owner_kind != DEVDOCK_FORWARD_OWNER
                || mapping.command_fingerprint
                    != port_forward_command_fingerprint(
                        workspace_name,
                        mapping.local_port,
                        mapping.remote_port,
                    )
            {
                return Err(RemoteError::PortForwardError(format!(
                    "managed forward for {} has unverifiable ownership metadata",
                    mapping.service_name
                )));
            }
            mappings.push(PortMapping {
                service_name: mapping.service_name.clone(),
                port_type: PortType::from_state_name(&mapping.port_type),
                bind_address: mapping.bind_address.clone(),
                local_port: mapping.local_port,
                remote_port: mapping.remote_port,
                process_pid: Some(mapping.process_pid),
            });
        }
        Ok(Self {
            workspace_name: workspace_name.to_string(),
            workspace_id: workspace_id.to_string(),
            devdock_id: manifest.instance_id.clone(),
            offset: manifest.port_base,
            best_effort_spawn: false,
            mappings,
        })
    }

    /// Start all port forwards and persist verified ownership before releasing
    /// each child handle.
    pub async fn start_all_forwards(&mut self, state: &StateManager) -> Result<()> {
        info!("Starting {} port forwards", self.mappings.len());

        let pid_dir = Self::get_pid_dir(&self.devdock_id)?;
        fs::create_dir_all(&pid_dir)
            .map_err(|e| RemoteError::PortForwardError(format!("Failed to create PID dir: {e}")))?;

        for mapping in &mut self.mappings {
            if let Some(existing) = prepare_existing_mapping(
                state,
                &self.devdock_id,
                &self.workspace_name,
                &self.workspace_id,
                &mapping.service_name,
                &mapping.port_type.state_name(),
                mapping.local_port,
                mapping.remote_port,
            )
            .await?
            {
                mapping.process_pid = existing.process_pid;
                debug!("Reusing verified managed forward for {}", mapping.service_name);
                continue;
            }
            let child = match coder::port_forward_single(
                &self.workspace_name,
                mapping.local_port,
                mapping.remote_port,
            ) {
                Ok(child) => child,
                Err(e) => {
                    if !self.best_effort_spawn {
                        return Err(e);
                    }
                    warn!("Failed to start port forward for {}: {}", mapping.service_name, e);
                    continue;
                }
            };

            let record = register_spawned_forward(
                state,
                &self.devdock_id,
                &self.workspace_name,
                &self.workspace_id,
                &mapping.service_name,
                &mapping.port_type.state_name(),
                mapping.local_port,
                mapping.remote_port,
                child,
            )
            .await?;
            let pid = record.process_pid.expect("managed forward has a PID");
            mapping.process_pid = Some(pid);

            // Keep the legacy PID layout for old binaries. It is compatibility
            // data only; exact database identity remains the kill authority.
            let pid_file =
                pid_dir.join(format!("{}_{:?}.pid", mapping.service_name, mapping.port_type));
            if let Err(error) = fs::write(&pid_file, pid.to_string()) {
                warn!("Failed to write legacy forward PID file {}: {error}", pid_file.display());
            }

            debug!(
                "Port forward started: {} {:?} -> {} (PID: {})",
                mapping.service_name, mapping.port_type, mapping.local_port, pid
            );
        }

        Ok(())
    }

    /// Stop all port forwards.
    pub async fn stop_all_forwards(&mut self, state: &StateManager) -> Result<()> {
        info!("Stopping all port forwards");

        let records = state.get_port_mapping_records(&self.devdock_id).await.map_err(|error| {
            RemoteError::PortForwardError(format!("failed to read managed forwards: {error}"))
        })?;
        for record in records {
            match stop_managed_port_forward(&record, &self.workspace_id, &self.workspace_name)? {
                ManagedForwardStopOutcome::Stopped | ManagedForwardStopOutcome::AlreadyExited => {
                    state
                        .remove_port_mapping_record(
                            &record.devdock_id,
                            &record.service_name,
                            &record.port_type,
                        )
                        .await
                        .map_err(|error| {
                            RemoteError::PortForwardError(format!(
                                "failed to remove stopped forward: {error}"
                            ))
                        })?;
                }
                ManagedForwardStopOutcome::Unverified(reason) => {
                    warn!("Not stopping unverified forward {}: {}", record.service_name, reason);
                }
            }
        }
        for mapping in &mut self.mappings {
            mapping.process_pid = None;
        }

        // Clean up PID directory
        let pid_dir = Self::get_pid_dir(&self.devdock_id)?;
        if pid_dir.exists() {
            let _ = fs::remove_dir_all(&pid_dir);
        }

        Ok(())
    }

    /// Get PID directory for this DevDock instance.
    fn get_pid_dir(devdock_id: &str) -> Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| {
            RemoteError::PortForwardError("Cannot determine home directory".to_string())
        })?;

        Ok(home.join(".devdock/port_forwards").join(devdock_id))
    }
}

/// Start and register one managed forward. If identity capture or durable
/// registration fails, the exact newly spawned child is terminated before the
/// error is returned.
#[allow(clippy::too_many_arguments)]
pub async fn start_managed_port_forward(
    state: &StateManager,
    devdock_id: &str,
    workspace_name: &str,
    workspace_id: &str,
    service_name: &str,
    port_type: &str,
    local_port: u16,
    remote_port: u16,
) -> Result<PortMappingRecord> {
    let port_type = canonical_port_type_name(port_type);
    if let Some(existing) = prepare_existing_mapping(
        state,
        devdock_id,
        workspace_name,
        workspace_id,
        service_name,
        &port_type,
        local_port,
        remote_port,
    )
    .await?
    {
        return Ok(existing);
    }
    let child = coder::port_forward_single(workspace_name, local_port, remote_port)?;
    register_spawned_forward(
        state,
        devdock_id,
        workspace_name,
        workspace_id,
        service_name,
        &port_type,
        local_port,
        remote_port,
        child,
    )
    .await
}

/// Manual forwarding variant that preserves the existing per-service log file.
#[allow(clippy::too_many_arguments)]
pub async fn start_managed_port_forward_logged(
    state: &StateManager,
    devdock_id: &str,
    workspace_name: &str,
    workspace_id: &str,
    service_name: &str,
    port_type: &str,
    local_port: u16,
    remote_port: u16,
    log_path: &Path,
) -> Result<PortMappingRecord> {
    let port_type = canonical_port_type_name(port_type);
    if let Some(existing) = prepare_existing_mapping(
        state,
        devdock_id,
        workspace_name,
        workspace_id,
        service_name,
        &port_type,
        local_port,
        remote_port,
    )
    .await?
    {
        return Ok(existing);
    }
    let child =
        coder::port_forward_single_logged(workspace_name, local_port, remote_port, log_path)?;
    register_spawned_forward(
        state,
        devdock_id,
        workspace_name,
        workspace_id,
        service_name,
        &port_type,
        local_port,
        remote_port,
        child,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn prepare_existing_mapping(
    state: &StateManager,
    devdock_id: &str,
    workspace_name: &str,
    workspace_id: &str,
    service_name: &str,
    port_type: &str,
    local_port: u16,
    remote_port: u16,
) -> Result<Option<PortMappingRecord>> {
    let existing = state
        .get_port_mapping_records(devdock_id)
        .await
        .map_err(|error| {
            RemoteError::PortForwardError(format!("failed to inspect persisted forwards: {error}"))
        })?
        .into_iter()
        .find(|mapping| {
            mapping.service_name == service_name
                && port_type_names_match(&mapping.port_type, port_type)
        });
    let Some(existing) = existing else {
        return Ok(None);
    };

    match verified_live_process(&existing, workspace_id, workspace_name) {
        Ok(Some(_)) if existing.local_port == local_port && existing.remote_port == remote_port => {
            Ok(Some(existing))
        }
        Ok(Some(_)) => {
            match stop_managed_port_forward(&existing, workspace_id, workspace_name)? {
                ManagedForwardStopOutcome::Stopped | ManagedForwardStopOutcome::AlreadyExited => {}
                ManagedForwardStopOutcome::Unverified(reason) => {
                    return Err(RemoteError::PortForwardError(format!(
                        "refusing to replace unverified forward for {service_name}: {reason}"
                    )));
                }
            }
            if let Err(error) = state
                .remove_port_mapping_record(devdock_id, service_name, &existing.port_type)
                .await
            {
                return Err(RemoteError::PortForwardError(format!(
                    "failed to remove replaced managed forward: {error}"
                )));
            }
            Ok(None)
        }
        Ok(None) => {
            if let Err(error) = state
                .remove_port_mapping_record(devdock_id, service_name, &existing.port_type)
                .await
            {
                return Err(RemoteError::PortForwardError(format!(
                    "failed to remove exited managed forward: {error}"
                )));
            }
            Ok(None)
        }
        Err(reason) => Err(RemoteError::PortForwardError(format!(
            "refusing to replace unverified forward for {service_name}: {reason}"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
async fn register_spawned_forward(
    state: &StateManager,
    devdock_id: &str,
    workspace_name: &str,
    workspace_id: &str,
    service_name: &str,
    port_type: &str,
    local_port: u16,
    remote_port: u16,
    mut child: Child,
) -> Result<PortMappingRecord> {
    let pid = child.id();
    let expected_fingerprint =
        port_forward_command_fingerprint(workspace_name, local_port, remote_port);
    let identity = match observe_process_identity(pid).and_then(|identity| {
        if identity.command_fingerprint == expected_fingerprint {
            Ok(identity)
        } else {
            Err("observed command does not match the requested forward".to_string())
        }
    }) {
        Ok(identity) => identity,
        Err(reason) => {
            terminate_spawned_child(&mut child);
            return Err(RemoteError::PortForwardError(format!(
                "could not verify ownership of newly spawned port forward PID {pid}: {reason}"
            )));
        }
    };

    let record = PortMappingRecord {
        devdock_id: devdock_id.to_string(),
        service_name: service_name.to_string(),
        port_type: port_type.to_string(),
        local_port,
        remote_port,
        process_pid: Some(pid),
        workspace_id: Some(workspace_id.to_string()),
        owner_kind: Some(DEVDOCK_FORWARD_OWNER.to_string()),
        process_start_time: Some(identity.start_time),
        command_fingerprint: Some(identity.command_fingerprint),
    };

    if let Err(error) = state.save_managed_port_mapping(&record).await {
        terminate_spawned_child(&mut child);
        return Err(RemoteError::PortForwardError(format!(
            "failed to register managed forward before releasing PID {pid}: {error}"
        )));
    }

    // Dropping std::process::Child does not terminate the process. Durable,
    // verified identity now owns its lifecycle; no leaked handle is required.
    drop(child);
    Ok(record)
}

pub fn port_forward_command_fingerprint(
    workspace_name: &str,
    local_port: u16,
    remote_port: u16,
) -> String {
    serde_json::to_string(&[
        "coder".to_string(),
        "port-forward".to_string(),
        workspace_name.to_string(),
        "--tcp".to_string(),
        coder::port_forward_tcp_spec(local_port, remote_port),
    ])
    .expect("forward command fingerprint is serializable")
}

fn normalized_forward_fingerprint(command: &[String]) -> Option<String> {
    let coder_index = command.iter().position(|argument| {
        Path::new(argument).file_name().and_then(|name| name.to_str()) == Some("coder")
    })?;
    let command = command.get(coder_index..)?;
    if command.len() != 5 || command[1] != "port-forward" || command[3] != "--tcp" {
        return None;
    }
    Some(
        serde_json::to_string(&[
            "coder".to_string(),
            command[1].clone(),
            command[2].clone(),
            command[3].clone(),
            command[4].clone(),
        ])
        .expect("observed command fingerprint is serializable"),
    )
}

fn observe_process_identity(pid: u32) -> std::result::Result<ProcessIdentity, String> {
    let mut last_reason = "process was not observable".to_string();
    let mut system = System::new();
    for _ in 0..20 {
        refresh_process_identity(&mut system, Pid::from_u32(pid));
        if let Some(process) = system.process(Pid::from_u32(pid)) {
            let executable = process.exe().and_then(Path::file_name).and_then(|name| name.to_str());
            if executable == Some("coder") {
                match normalized_forward_fingerprint(process.cmd()) {
                    Some(command_fingerprint) => {
                        return Ok(ProcessIdentity {
                            start_time: process.start_time(),
                            command_fingerprint,
                        });
                    }
                    None => {
                        last_reason = format!(
                            "process command does not match coder port-forward: {:?}",
                            process.cmd()
                        );
                    }
                }
            } else {
                last_reason = format!(
                    "process executable is {}, not coder",
                    executable.unwrap_or("unavailable")
                );
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(last_reason)
}

fn refresh_process_identity(system: &mut System, pid: Pid) -> bool {
    // sysinfo's default single-process refresh intentionally omits argv. Request it explicitly:
    // the normalized command is part of both registration and kill authority.
    system.refresh_process_specifics(
        pid,
        ProcessRefreshKind::new().with_cmd(UpdateKind::Always).with_exe(UpdateKind::Always),
    )
}

fn terminate_spawned_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn wait_for_process_exit(pid: Pid, start_time: u64) -> bool {
    let mut system = System::new();
    for _ in 0..20 {
        system.refresh_process(pid);
        match system.process(pid) {
            None => return true,
            Some(process)
                if matches!(process.status(), ProcessStatus::Dead | ProcessStatus::Zombie) =>
            {
                return true;
            }
            Some(process) if process.start_time() != start_time => return true,
            Some(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    false
}

/// Stop only when persisted ownership, immutable workspace identity, process
/// start time, and normalized command all match the live PID.
fn verified_live_process(
    mapping: &PortMappingRecord,
    expected_workspace_id: &str,
    expected_workspace_name: &str,
) -> std::result::Result<Option<VerifiedLiveProcess>, String> {
    let Some(pid) = mapping.process_pid else {
        return Ok(None);
    };
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    refresh_process_identity(&mut system, pid);
    let Some(process) = system.process(pid) else {
        return Ok(None);
    };
    if matches!(process.status(), ProcessStatus::Dead | ProcessStatus::Zombie) {
        return Ok(None);
    }

    if !mapping.is_devdock_managed() {
        return Err("legacy or external mapping has incomplete ownership identity".to_string());
    }
    if mapping.workspace_id.as_deref() != Some(expected_workspace_id) {
        return Err("workspace identity does not match the owning session".to_string());
    }
    let expected_fingerprint = port_forward_command_fingerprint(
        expected_workspace_name,
        mapping.local_port,
        mapping.remote_port,
    );
    if mapping.command_fingerprint.as_deref() != Some(expected_fingerprint.as_str()) {
        return Err("stored command fingerprint does not match the resolved mapping".to_string());
    }
    if mapping.process_start_time != Some(process.start_time()) {
        return Err("PID was reused by a different process".to_string());
    }
    if process.exe().and_then(Path::file_name).and_then(|name| name.to_str()) != Some("coder") {
        return Err("live process executable is not Coder".to_string());
    }
    if normalized_forward_fingerprint(process.cmd()).as_deref()
        != Some(expected_fingerprint.as_str())
    {
        return Err("live process command does not match the managed forward".to_string());
    }

    Ok(Some(VerifiedLiveProcess { system, pid }))
}

pub fn managed_port_forward_status(
    mapping: &PortMappingRecord,
    expected_workspace_id: &str,
    expected_workspace_name: &str,
) -> ManagedForwardStatus {
    match verified_live_process(mapping, expected_workspace_id, expected_workspace_name) {
        Ok(Some(_)) => ManagedForwardStatus::Active,
        Ok(None) => ManagedForwardStatus::Exited,
        Err(reason) => ManagedForwardStatus::Unverified(reason),
    }
}

pub fn stop_managed_port_forward(
    mapping: &PortMappingRecord,
    expected_workspace_id: &str,
    expected_workspace_name: &str,
) -> Result<ManagedForwardStopOutcome> {
    let verified =
        match verified_live_process(mapping, expected_workspace_id, expected_workspace_name) {
            Ok(Some(verified)) => verified,
            Ok(None) => return Ok(ManagedForwardStopOutcome::AlreadyExited),
            Err(reason) => return Ok(ManagedForwardStopOutcome::Unverified(reason)),
        };
    let pid = verified.pid.as_u32();
    let process = verified
        .system
        .process(verified.pid)
        .expect("verified process remains in the captured system snapshot");
    let start_time = mapping.process_start_time.expect("managed mapping has a start time");
    let term_result = process.kill_with(Signal::Term);
    let signal_sent = term_result.unwrap_or_else(|| process.kill());
    if !signal_sent {
        return Err(RemoteError::PortForwardError(format!(
            "failed to terminate verified managed forward PID {pid}"
        )));
    }
    if wait_for_process_exit(verified.pid, start_time) {
        return Ok(ManagedForwardStopOutcome::Stopped);
    }

    // Re-verify immediately before escalation so PID reuse can never authorize
    // a hard kill of a different process.
    let verified =
        match verified_live_process(mapping, expected_workspace_id, expected_workspace_name) {
            Ok(Some(verified)) => verified,
            Ok(None) => return Ok(ManagedForwardStopOutcome::Stopped),
            Err(reason) => return Ok(ManagedForwardStopOutcome::Unverified(reason)),
        };
    let process = verified
        .system
        .process(verified.pid)
        .expect("re-verified process remains in the captured system snapshot");
    if !process.kill() || !wait_for_process_exit(verified.pid, start_time) {
        return Err(RemoteError::PortForwardError(format!(
            "verified managed forward PID {pid} did not exit"
        )));
    }
    Ok(ManagedForwardStopOutcome::Stopped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_profile(dir: &TempDir, name: &str, yaml: &str) -> PathBuf {
        let profiles_dir = dir.path().join("devdock/profiles");
        fs::create_dir_all(&profiles_dir).unwrap();
        let profile_path = profiles_dir.join(format!("{name}.yaml"));
        fs::write(&profile_path, yaml).unwrap();
        profile_path
    }

    #[test]
    fn from_profile_always_includes_tilt_and_nginx() {
        let tmp = TempDir::new().unwrap();
        let profile_path = setup_profile(&tmp, "empty", "services: {}");

        let manager = PortForwardManager::from_profile(
            &profile_path,
            "workspace",
            "workspace-id",
            "devdock-id",
            1000,
        )
        .unwrap();

        assert!(manager.mappings.iter().any(|mapping| {
            mapping.service_name == "tilt"
                && mapping.port_type == PortType::TiltUI
                && mapping.local_port == 11350
                && mapping.remote_port == 10350
        }));
        assert!(manager.mappings.iter().any(|mapping| {
            mapping.service_name == "nginx"
                && mapping.port_type == PortType::Service
                && mapping.local_port == 8321
                && mapping.remote_port == 8321
        }));
    }

    #[test]
    fn manual_commands_include_supporting_forwards_and_selected_services() {
        let tmp = TempDir::new().unwrap();
        let profile_path = setup_profile(
            &tmp,
            "selected",
            "services:\n  qe:\n    type: local\n    port: 7960\n  crawler:\n    type: local\n    port: 7961\n",
        );

        let commands = PortForwardManager::manual_forward_commands(
            &profile_path,
            "dev-1afa81a3",
            1000,
            Some("qe"),
        )
        .unwrap();

        assert!(commands.iter().any(|command| {
            command == "coder port-forward dev-1afa81a3 --tcp 127.0.0.1:11350:10350"
        }));
        assert!(commands.iter().any(|command| {
            command == "coder port-forward dev-1afa81a3 --tcp 127.0.0.1:8321:8321"
        }));
        assert!(commands.iter().any(|command| {
            command == "coder port-forward dev-1afa81a3 --tcp 127.0.0.1:8960:7960"
        }));
        assert!(!commands.iter().any(|command| command.contains(":8961:7961")));
    }

    #[test]
    fn from_profile_dedupes_nginx_when_profile_lists_it() {
        let tmp = TempDir::new().unwrap();
        let profile_path = setup_profile(
            &tmp,
            "default",
            "services:\n  nginx:\n    type: local\n    port: 8321\n",
        );

        let manager = PortForwardManager::from_profile(
            &profile_path,
            "workspace",
            "workspace-id",
            "devdock-id",
            0,
        )
        .unwrap();

        assert_eq!(
            manager
                .mappings
                .iter()
                .filter(|mapping| mapping.service_name == "nginx"
                    && mapping.port_type == PortType::Service)
                .count(),
            1
        );
    }

    #[test]
    fn from_profile_rejects_unresolved_project_before_accepting_mappings() {
        std::env::remove_var("DEVDock_PORT_FORWARD_PROJECT");
        let tmp = TempDir::new().unwrap();
        let profile_path = setup_profile(
            &tmp,
            "unresolved",
            "project: ${DEVDock_PORT_FORWARD_PROJECT}\nservices: {}\n",
        );

        let error = match PortForwardManager::from_profile(
            &profile_path,
            "workspace",
            "workspace-id",
            "devdock-id",
            0,
        ) {
            Ok(_) => panic!("unresolved project identity must fail before mappings are accepted"),
            Err(error) => error,
        };

        let message = error.to_string();
        assert!(message.contains("DEVDock_PORT_FORWARD_PROJECT"));
        assert!(!message.contains("${DEVDock_PORT_FORWARD_PROJECT}"));
        assert!(matches!(error, RemoteError::ConfigSyncError(_)));
    }

    #[test]
    fn fingerprint_normalizes_the_coder_executable_path() {
        let command = vec![
            "/opt/coder/bin/coder".to_string(),
            "port-forward".to_string(),
            "workspace".to_string(),
            "--tcp".to_string(),
            "127.0.0.1:11350:10350".to_string(),
        ];
        assert_eq!(
            normalized_forward_fingerprint(&command).as_deref(),
            Some(port_forward_command_fingerprint("workspace", 11350, 10350).as_str())
        );
    }

    #[test]
    fn port_type_names_use_one_canonical_persisted_spelling() {
        assert_eq!(canonical_port_type_name("service"), "Service");
        assert_eq!(canonical_port_type_name("DEBUG"), "Debug");
        assert_eq!(canonical_port_type_name("tilt_ui"), "TiltUI");
        assert!(port_type_names_match("service", "Service"));
        assert!(port_type_names_match("tilt_ui", "TiltUI"));
        assert!(!port_type_names_match("Service", "Debug"));
    }

    #[test]
    fn legacy_mapping_is_never_kill_authority() {
        let mapping = PortMappingRecord {
            devdock_id: "devdock-id".to_string(),
            service_name: "tilt".to_string(),
            port_type: "tilt_ui".to_string(),
            local_port: 11350,
            remote_port: 10350,
            process_pid: Some(std::process::id()),
            workspace_id: None,
            owner_kind: None,
            process_start_time: None,
            command_fingerprint: None,
        };
        assert!(matches!(
            stop_managed_port_forward(&mapping, "workspace-id", "workspace").unwrap(),
            ManagedForwardStopOutcome::Unverified(_)
        ));
    }

    #[test]
    fn live_pid_with_wrong_command_is_never_kill_authority() {
        let pid = std::process::id();
        let mut system = System::new();
        system.refresh_process(Pid::from_u32(pid));
        let start_time = system.process(Pid::from_u32(pid)).unwrap().start_time();
        let mapping = PortMappingRecord {
            devdock_id: "devdock-id".to_string(),
            service_name: "tilt".to_string(),
            port_type: "TiltUI".to_string(),
            local_port: 11350,
            remote_port: 10350,
            process_pid: Some(pid),
            workspace_id: Some("workspace-id".to_string()),
            owner_kind: Some(DEVDOCK_FORWARD_OWNER.to_string()),
            process_start_time: Some(start_time),
            command_fingerprint: Some(port_forward_command_fingerprint("workspace", 11350, 10350)),
        };

        assert!(matches!(
            managed_port_forward_status(&mapping, "workspace-id", "workspace"),
            ManagedForwardStatus::Unverified(_)
        ));
        assert!(matches!(
            stop_managed_port_forward(&mapping, "workspace-id", "workspace").unwrap(),
            ManagedForwardStopOutcome::Unverified(_)
        ));
    }

    #[test]
    fn manifest_reconstruction_rejects_non_loopback_binding() {
        let manifest = SessionManifestV1 {
            schema: "devdock.session/v1".to_string(),
            instance_id: "devdock-id".to_string(),
            canonical_worktree: "/repo".to_string(),
            planned_workspace_name: "workspace".to_string(),
            workspace_id: Some("workspace-id".to_string()),
            workspace_name: Some("workspace".to_string()),
            profile: "default".to_string(),
            branch: "master".to_string(),
            port_base: 1000,
            port_mappings: vec![devdock_state::SessionPortMappingV1 {
                service_name: "tilt".to_string(),
                port_type: "tilt_ui".to_string(),
                bind_address: "0.0.0.0".to_string(),
                local_port: 11350,
                remote_port: 10350,
                process_pid: 42,
                workspace_id: "workspace-id".to_string(),
                owner_kind: DEVDOCK_FORWARD_OWNER.to_string(),
                process_start_time: 1,
                command_fingerprint: port_forward_command_fingerprint("workspace", 11350, 10350),
            }],
            created_at: 1,
            updated_at: 1,
        };
        assert!(PortForwardManager::from_manifest(&manifest).is_err());
    }

    #[test]
    fn manifest_reconstructs_resolved_mapping_without_profile() {
        let manifest = SessionManifestV1 {
            schema: "devdock.session/v1".to_string(),
            instance_id: "devdock-id".to_string(),
            canonical_worktree: "/repo".to_string(),
            planned_workspace_name: "workspace".to_string(),
            workspace_id: Some("workspace-id".to_string()),
            workspace_name: Some("workspace".to_string()),
            profile: "deleted-profile".to_string(),
            branch: "master".to_string(),
            port_base: 1000,
            port_mappings: vec![devdock_state::SessionPortMappingV1 {
                service_name: "tilt".to_string(),
                port_type: "TiltUI".to_string(),
                bind_address: LOOPBACK_BIND_ADDRESS.to_string(),
                local_port: 11350,
                remote_port: 10350,
                process_pid: 42,
                workspace_id: "workspace-id".to_string(),
                owner_kind: DEVDOCK_FORWARD_OWNER.to_string(),
                process_start_time: 1,
                command_fingerprint: port_forward_command_fingerprint("workspace", 11350, 10350),
            }],
            created_at: 1,
            updated_at: 1,
        };

        let manager = PortForwardManager::from_manifest(&manifest).unwrap();
        assert_eq!(manager.mappings.len(), 1);
        assert_eq!(manager.mappings[0].service_name, "tilt");
        assert_eq!(manager.mappings[0].bind_address, LOOPBACK_BIND_ADDRESS);
        assert_eq!(manager.mappings[0].process_pid, Some(42));
    }
}
