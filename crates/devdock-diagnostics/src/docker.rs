//! Docker container information collection.

use crate::{DiagnosticsError, Result};
use std::collections::HashMap;
use std::process::Command;

pub use crate::system::DockerInfo;

/// Get information about running Docker containers.
pub fn get_docker_info() -> Result<DockerInfo> {
    let output =
        Command::new("docker").args(["ps", "--format", "{{.Names}}\t{{.Status}}"]).output();

    match output {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut containers = HashMap::new();

            for line in stdout.lines() {
                if line.trim().is_empty() {
                    continue;
                }

                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 2 {
                    containers.insert(parts[0].to_string(), parts[1].to_string());
                }
            }

            Ok(DockerInfo { containers })
        }
        Ok(_) => Ok(DockerInfo {
            containers: {
                let mut map = HashMap::new();
                map.insert("error".to_string(), "Docker command failed".to_string());
                map
            },
        }),
        Err(e) => Err(DiagnosticsError::SystemInfo(format!("Failed to run docker command: {}", e))),
    }
}

/// Check if Tilt process is running.
pub fn check_tilt_running() -> Result<Option<u32>> {
    use sysinfo::System;

    let mut sys = System::new_all();
    sys.refresh_processes();

    for (pid, process) in sys.processes() {
        let name = process.name().to_lowercase();
        if name.contains("tilt") {
            return Ok(Some(pid.as_u32()));
        }
    }

    Ok(None)
}
