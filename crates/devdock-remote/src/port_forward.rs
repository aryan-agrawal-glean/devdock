//! Port forwarding management for remote workspaces.

use crate::{coder, RemoteError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
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

/// Represents a port mapping from remote to local with offset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    pub service_name: String,
    pub port_type: PortType,
    pub local_port: u16,
    pub remote_port: u16,
    pub process_pid: Option<u32>,
}

/// Manages port forwarding for a remote workspace.
pub struct PortForwardManager {
    workspace_name: String,
    devdock_id: String,
    #[allow(dead_code)]
    offset: u16,
    pub mappings: Vec<PortMapping>,
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
        devdock_id: &str,
        offset: u16,
    ) -> Result<Self> {
        info!("Parsing profile for port mappings: {:?}", profile_path);

        let contents = fs::read_to_string(profile_path)
            .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to read profile: {e}")))?;

        let profile: Profile = serde_yaml::from_str(&contents)
            .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to parse profile: {e}")))?;

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
            devdock_id: devdock_id.to_string(),
            offset,
            mappings,
        })
    }

    /// Start all port forwards.
    pub fn start_all_forwards(&mut self) -> Result<()> {
        info!("Starting {} port forwards", self.mappings.len());

        let pid_dir = Self::get_pid_dir(&self.devdock_id)?;
        fs::create_dir_all(&pid_dir)
            .map_err(|e| RemoteError::PortForwardError(format!("Failed to create PID dir: {e}")))?;

        for mapping in &mut self.mappings {
            match coder::port_forward_single(
                &self.workspace_name,
                mapping.local_port,
                mapping.remote_port,
            ) {
                Ok(child) => {
                    let pid = child.id();
                    mapping.process_pid = Some(pid);

                    // Write PID file
                    let pid_file = pid_dir
                        .join(format!("{}_{:?}.pid", mapping.service_name, mapping.port_type));
                    let _ = fs::write(&pid_file, pid.to_string());

                    debug!(
                        "Port forward started: {} {:?} -> {} (PID: {})",
                        mapping.service_name, mapping.port_type, mapping.local_port, pid
                    );

                    // Keep the child process alive - don't let it be dropped
                    std::mem::forget(child);
                }
                Err(e) => {
                    warn!("Failed to start port forward for {}: {}", mapping.service_name, e);
                }
            }
        }

        Ok(())
    }

    /// Stop all port forwards.
    pub fn stop_all_forwards(&mut self) -> Result<()> {
        info!("Stopping all port forwards");

        for mapping in &mut self.mappings {
            if let Some(pid) = mapping.process_pid {
                let _ = coder::kill_port_forward(pid);
                mapping.process_pid = None;
            }
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

// Note: We don't implement Drop to stop port forwards automatically
// because we use std::mem::forget on the Child processes to keep them alive.
// Port forwards should be stopped explicitly via `devdock stop` or by killing the PIDs.

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

        let manager =
            PortForwardManager::from_profile(&profile_path, "workspace", "devdock-id", 1000)
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
    fn from_profile_dedupes_nginx_when_profile_lists_it() {
        let tmp = TempDir::new().unwrap();
        let profile_path = setup_profile(
            &tmp,
            "default",
            "services:\n  nginx:\n    type: local\n    port: 8321\n",
        );

        let manager =
            PortForwardManager::from_profile(&profile_path, "workspace", "devdock-id", 0).unwrap();

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
}
