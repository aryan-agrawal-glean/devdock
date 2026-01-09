//! Statistics collection for startup monitoring.
//!
//! Collects host stats, docker stats, and resource bootup times.

use crate::{Result, SessionTiming, TiltClient, TiltResource};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::process::Command;
use tracing::warn;

/// Host system statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostStats {
    pub devdock_id: Option<String>,
    pub machine_id: String,
    pub platform: String,
    pub architecture: String,
    pub cpu_percent: f64,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub memory_percent: f64,
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub disk_percent: f64,
    pub timestamp: DateTime<Utc>,
}

/// Per-resource startup statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceStats {
    pub name: String,
    pub resource_type: String,
    pub bootup_time_seconds: Option<f64>,
    pub state: String,
    pub load_order: i32,
}

/// Collector for startup statistics.
#[derive(Debug, Default)]
pub struct StatsCollector {
    tilt_client: TiltClient,
}

impl StatsCollector {
    pub fn new() -> Self {
        StatsCollector {
            tilt_client: TiltClient::new(),
        }
    }

    /// Collect host system statistics.
    pub fn collect_host_stats(&self) -> Result<HostStats> {
        let devdock_id = SessionTiming::get_devdock_id();
        let machine_id = SessionTiming::get_machine_id();

        let (cpu_percent, memory_used, memory_total, memory_percent) = self.get_memory_stats();
        let (disk_used, disk_total, disk_percent) = self.get_disk_stats();

        Ok(HostStats {
            devdock_id,
            machine_id,
            platform: std::env::consts::OS.to_string(),
            architecture: std::env::consts::ARCH.to_string(),
            cpu_percent,
            memory_used_bytes: memory_used,
            memory_total_bytes: memory_total,
            memory_percent,
            disk_used_bytes: disk_used,
            disk_total_bytes: disk_total,
            disk_percent,
            timestamp: Utc::now(),
        })
    }

    /// Collect per-resource startup statistics.
    pub fn collect_resource_stats(&self) -> Result<Vec<ResourceStats>> {
        let resources = self.tilt_client.get_monitored_resources()?;
        let mut stats = Vec::new();

        for resource in resources {
            let bootup_time = self.calculate_bootup_time(&resource);

            stats.push(ResourceStats {
                name: resource.name.clone(),
                resource_type: format!("{:?}", resource.resource_type),
                bootup_time_seconds: bootup_time,
                state: format!("{:?}", resource.status),
                load_order: 0, // Would need to extract from tilt JSON
            });
        }

        Ok(stats)
    }

    fn calculate_bootup_time(&self, _resource: &TiltResource) -> Option<f64> {
        // Bootup time calculation requires access to buildHistory and conditions
        // from the full tilt JSON. For now, return None.
        // Full implementation would parse the timestamps from tilt's JSON output.
        None
    }

    #[cfg(target_os = "macos")]
    fn get_memory_stats(&self) -> (f64, u64, u64, f64) {
        let output = Command::new("vm_stat").output();

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut pages_free = 0u64;
            let mut pages_active = 0u64;
            let mut pages_inactive = 0u64;
            let mut pages_wired = 0u64;

            for line in stdout.lines() {
                if line.contains("Pages free:") {
                    pages_free = Self::parse_vm_stat_value(line);
                } else if line.contains("Pages active:") {
                    pages_active = Self::parse_vm_stat_value(line);
                } else if line.contains("Pages inactive:") {
                    pages_inactive = Self::parse_vm_stat_value(line);
                } else if line.contains("Pages wired down:") {
                    pages_wired = Self::parse_vm_stat_value(line);
                }
            }

            let page_size = 4096u64;
            let total = (pages_free + pages_active + pages_inactive + pages_wired) * page_size;
            let used = (pages_active + pages_wired) * page_size;
            let percent = if total > 0 {
                (used as f64 / total as f64) * 100.0
            } else {
                0.0
            };

            (0.0, used, total, percent)
        } else {
            (0.0, 0, 0, 0.0)
        }
    }

    #[cfg(target_os = "macos")]
    fn parse_vm_stat_value(line: &str) -> u64 {
        line.split(':')
            .nth(1)
            .and_then(|v| v.trim().trim_end_matches('.').parse().ok())
            .unwrap_or(0)
    }

    #[cfg(target_os = "linux")]
    fn get_memory_stats(&self) -> (f64, u64, u64, f64) {
        if let Ok(contents) = std::fs::read_to_string("/proc/meminfo") {
            let mut total = 0u64;
            let mut available = 0u64;

            for line in contents.lines() {
                if line.starts_with("MemTotal:") {
                    total = Self::parse_meminfo_value(line);
                } else if line.starts_with("MemAvailable:") {
                    available = Self::parse_meminfo_value(line);
                }
            }

            let used = total.saturating_sub(available);
            let percent = if total > 0 {
                (used as f64 / total as f64) * 100.0
            } else {
                0.0
            };

            (0.0, used, total, percent)
        } else {
            (0.0, 0, 0, 0.0)
        }
    }

    #[cfg(target_os = "linux")]
    fn parse_meminfo_value(line: &str) -> u64 {
        line.split_whitespace()
            .nth(1)
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v * 1024) // Convert from kB to bytes
            .unwrap_or(0)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn get_memory_stats(&self) -> (f64, u64, u64, f64) {
        (0.0, 0, 0, 0.0)
    }

    fn get_disk_stats(&self) -> (u64, u64, f64) {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        let devdock_path = home.join(".devdock");

        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::mem::MaybeUninit;

            let path = devdock_path.to_string_lossy();
            if let Ok(cpath) = CString::new(path.as_bytes()) {
                let mut stat: MaybeUninit<libc::statvfs> = MaybeUninit::uninit();
                unsafe {
                    if libc::statvfs(cpath.as_ptr(), stat.as_mut_ptr()) == 0 {
                        let stat = stat.assume_init();
                        let total = (stat.f_blocks as u64) * (stat.f_frsize as u64);
                        let free = (stat.f_bfree as u64) * (stat.f_frsize as u64);
                        let used = total - free;
                        let percent = if total > 0 {
                            (used as f64 / total as f64) * 100.0
                        } else {
                            0.0
                        };
                        return (used, total, percent);
                    }
                }
            }
        }

        (0, 0, 0.0)
    }

    /// Get docker container statistics.
    pub fn collect_docker_stats(&self) -> Result<Vec<DockerContainerStats>> {
        let output = Command::new("docker")
            .args(["stats", "--no-stream", "--format", "{{json .}}"])
            .output()?;

        if !output.status.success() {
            warn!("Failed to get docker stats");
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut stats = Vec::new();

        for line in stdout.lines() {
            if line.is_empty() {
                continue;
            }

            if let Ok(container) = serde_json::from_str::<DockerStatsJson>(line) {
                stats.push(DockerContainerStats {
                    container_id: container.id,
                    name: container.name,
                    cpu_percent: Self::parse_percent(&container.cpu_perc),
                    memory_usage_bytes: Self::parse_size(&container.mem_usage.split('/').next().unwrap_or("0")),
                    memory_limit_bytes: Self::parse_size(&container.mem_usage.split('/').nth(1).unwrap_or("0")),
                    memory_percent: Self::parse_percent(&container.mem_perc),
                    pids: container.pids.parse().unwrap_or(0),
                    timestamp: Utc::now(),
                });
            }
        }

        Ok(stats)
    }

    fn parse_percent(s: &str) -> f64 {
        s.trim_end_matches('%').parse().unwrap_or(0.0)
    }

    fn parse_size(s: &str) -> u64 {
        let s = s.trim();
        let (num, unit) = if let Some(idx) = s.find(|c: char| !c.is_ascii_digit() && c != '.') {
            (&s[..idx], &s[idx..])
        } else {
            (s, "")
        };

        let value: f64 = num.parse().unwrap_or(0.0);
        let multiplier = match unit.trim().to_uppercase().as_str() {
            "B" => 1.0,
            "KIB" | "KB" => 1024.0,
            "MIB" | "MB" => 1024.0 * 1024.0,
            "GIB" | "GB" => 1024.0 * 1024.0 * 1024.0,
            _ => 1.0,
        };

        (value * multiplier) as u64
    }
}

/// Docker stats JSON format from `docker stats --format "{{json .}}"`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerStatsJson {
    #[serde(rename = "ID")]
    id: String,
    name: String,
    #[serde(rename = "CPUPerc")]
    cpu_perc: String,
    #[serde(rename = "MemUsage")]
    mem_usage: String,
    #[serde(rename = "MemPerc")]
    mem_perc: String,
    #[serde(rename = "PIDs")]
    pids: String,
}

/// Per-container Docker statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerContainerStats {
    pub container_id: String,
    pub name: String,
    pub cpu_percent: f64,
    pub memory_usage_bytes: u64,
    pub memory_limit_bytes: u64,
    pub memory_percent: f64,
    pub pids: u32,
    pub timestamp: DateTime<Utc>,
}

