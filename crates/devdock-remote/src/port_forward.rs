//! Port forwarding management for remote workspaces.

use crate::{coder, RemoteError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::fs;
use tracing::{debug, info, warn};

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
}

/// Profile structure (simplified).
#[derive(Debug, Deserialize)]
struct Profile {
    services: Option<HashMap<String, ProfileService>>,
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
            .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to read profile: {}", e)))?;
        
        let profile: Profile = serde_yaml::from_str(&contents)
            .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to parse profile: {}", e)))?;
        
        let mut mappings = Vec::new();
        
        // Add Tilt UI port (always present)
        mappings.push(PortMapping {
            service_name: "tilt".to_string(),
            port_type: PortType::TiltUI,
            local_port: offset + 10350,
            remote_port: 10350,
            process_pid: None,
        });
        
        // Extract service ports
        if let Some(services) = profile.services {
            for (service_name, service) in services {
                // Add main service port
                if let Some(port) = service.port {
                    mappings.push(PortMapping {
                        service_name: service_name.clone(),
                        port_type: PortType::Service,
                        local_port: offset + port,
                        remote_port: port,
                        process_pid: None,
                    });
                }
                
                // Add debug port if present
                if let Some(debug_port) = service.debug_port {
                    mappings.push(PortMapping {
                        service_name: service_name.clone(),
                        port_type: PortType::Debug,
                        local_port: offset + debug_port,
                        remote_port: debug_port,
                        process_pid: None,
                    });
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
            .map_err(|e| RemoteError::PortForwardError(format!("Failed to create PID dir: {}", e)))?;
        
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
                    let pid_file = pid_dir.join(format!(
                        "{}_{:?}.pid",
                        mapping.service_name,
                        mapping.port_type
                    ));
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
        let home = dirs::home_dir()
            .ok_or_else(|| RemoteError::PortForwardError("Cannot determine home directory".to_string()))?;
        
        Ok(home.join(".devdock/port_forwards").join(devdock_id))
    }
}

// Note: We don't implement Drop to stop port forwards automatically
// because we use std::mem::forget on the Child processes to keep them alive.
// Port forwards should be stopped explicitly via `devdock stop` or by killing the PIDs.

