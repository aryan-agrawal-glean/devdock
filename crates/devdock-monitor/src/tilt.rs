//! Tilt CLI client for querying resource status.

use crate::{MonitorError, Result, TiltResource};
use std::process::Command;
use tracing::{debug, warn};

/// Client for interacting with the Tilt CLI.
#[derive(Debug, Clone, Default)]
pub struct TiltClient;

impl TiltClient {
    pub fn new() -> Self {
        TiltClient
    }

    /// Check if Tilt is running by trying to get resources.
    pub fn is_running(&self) -> bool {
        Command::new("tilt")
            .args(["get", "uiresource", "-o", "json"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get all UI resources from Tilt.
    pub fn get_resources(&self) -> Result<Vec<TiltResource>> {
        let output = Command::new("tilt")
            .args(["get", "uiresource", "-o", "json"])
            .output()
            .map_err(|e| MonitorError::TiltCommand(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("connection refused") || stderr.contains("not running") {
                return Err(MonitorError::TiltNotRunning);
            }
            return Err(MonitorError::TiltCommand(stderr.to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        self.parse_resources(&stdout)
    }

    /// Get only resources that should be monitored during startup.
    pub fn get_monitored_resources(&self) -> Result<Vec<TiltResource>> {
        let all_resources = self.get_resources()?;
        Ok(all_resources.into_iter().filter(|r| r.should_monitor()).collect())
    }

    /// Get logs for a specific resource.
    pub fn get_logs(&self, resource_name: &str) -> Result<String> {
        let output = Command::new("tilt")
            .args(["logs", resource_name])
            .output()
            .map_err(|e| MonitorError::TiltCommand(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to get logs for {}: {}", resource_name, stderr);
            return Ok(format!("Error getting logs: {}", stderr));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn parse_resources(&self, json_str: &str) -> Result<Vec<TiltResource>> {
        let data: serde_json::Value =
            serde_json::from_str(json_str).map_err(|e| MonitorError::ParseError(e.to_string()))?;

        let items = data
            .get("items")
            .and_then(|i| i.as_array())
            .ok_or_else(|| MonitorError::ParseError("Missing 'items' array".to_string()))?;

        let resources: Vec<TiltResource> = items
            .iter()
            .filter_map(|item| match TiltResource::from_json(item) {
                Some(r) => {
                    if r.name == "(Tiltfile)" {
                        None
                    } else {
                        debug!("Parsed resource: {} -> {:?}", r.name, r.status);
                        Some(r)
                    }
                }
                None => {
                    warn!("Failed to parse resource: {:?}", item.get("metadata"));
                    None
                }
            })
            .collect();

        Ok(resources)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_resources() {
        let json = r#"{
            "items": [
                {
                    "metadata": {
                        "name": "test-service",
                        "labels": ["service"]
                    },
                    "status": {
                        "runtimeStatus": "ok",
                        "updateStatus": "ok",
                        "conditions": [{"type": "Ready", "status": "True"}]
                    }
                }
            ]
        }"#;

        let client = TiltClient::new();
        let resources = client.parse_resources(json).unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].name, "test-service");
    }
}
