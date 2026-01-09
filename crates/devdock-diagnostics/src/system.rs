//! System information collection.

use crate::{DiagnosticsError, Result};
use chrono::Local;
use serde::{Deserialize, Serialize};
use sysinfo::System;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub timestamp: String,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub disk: DiskInfo,
    pub docker: DockerInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuInfo {
    pub percent: f32,
    pub cores: usize,
    pub physical_cores: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total: u64,
    pub available: u64,
    pub percent: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskInfo {
    pub total: u64,
    pub used: u64,
    pub free: u64,
    pub percent: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerInfo {
    #[serde(flatten)]
    pub containers: std::collections::HashMap<String, String>,
}

/// Collect comprehensive system information.
pub fn collect_system_info() -> Result<SystemInfo> {
    let mut sys = System::new_all();
    sys.refresh_all();

    // CPU information
    let cpu_percent = sys.global_cpu_info().cpu_usage();
    let cores = sys.cpus().len();
    let physical_cores = sys.physical_core_count().unwrap_or(cores);

    // Memory information
    let mem_total = sys.total_memory();
    let mem_available = sys.available_memory();
    let mem_percent = ((mem_total - mem_available) as f32 / mem_total as f32) * 100.0;

    // Disk information
    let disk = get_disk_info()?;

    // Docker information
    let docker = crate::docker::get_docker_info()?;

    Ok(SystemInfo {
        timestamp: Local::now().to_rfc3339(),
        cpu: CpuInfo {
            percent: cpu_percent,
            cores,
            physical_cores,
        },
        memory: MemoryInfo {
            total: mem_total,
            available: mem_available,
            percent: mem_percent,
        },
        disk,
        docker,
    })
}

fn get_disk_info() -> Result<DiskInfo> {
    use sysinfo::Disks;

    let disks = Disks::new_with_refreshed_list();

    // Find root disk
    for disk in &disks {
        if disk.mount_point().to_str() == Some("/") {
            let total = disk.total_space();
            let available = disk.available_space();
            let used = total - available;
            let percent = (used as f32 / total as f32) * 100.0;

            return Ok(DiskInfo {
                total,
                used,
                free: available,
                percent,
            });
        }
    }

    // Fallback: use first disk
    if let Some(disk) = disks.iter().next() {
        let total = disk.total_space();
        let available = disk.available_space();
        let used = total - available;
        let percent = (used as f32 / total as f32) * 100.0;

        return Ok(DiskInfo {
            total,
            used,
            free: available,
            percent,
        });
    }

    Err(DiagnosticsError::SystemInfo("No disk found".to_string()))
}
