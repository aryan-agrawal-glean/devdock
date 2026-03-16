//! DevDock BigQuery Sync Plugin
//!
//! Syncs telemetry data (collected by devdock core) to Google BigQuery.
//! All config (project, dataset) comes from devdock.toml via initialize().
//!
//! Lifecycle hooks:
//! - `after_tilt` — session started event
//! - `after_ready` — bootstrap complete event
//! - `before_stop` — session end event
//!
//! Install: cargo install --path plugins/telemetry/bigquery-sync
//! Config:
//!   [telemetry]
//!   backend = "external"
//!   plugin = "devdock-plugin-bigquery-sync"
//!   project = "your-gcp-project"
//!   dataset = "devdock"

use devdock_plugin_sdk::{ok, Plugin};
use serde_json::Value;
use std::process::Command;

#[derive(Default)]
struct BigQuerySync {
    project_id: String,
    dataset: String,
}

impl BigQuerySync {
    /// Get access token via gcloud CLI.
    fn get_access_token(&self) -> Option<String> {
        let output = Command::new("gcloud")
            .args(["auth", "application-default", "print-access-token"])
            .output()
            .ok()?;

        if output.status.success() {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !token.is_empty() {
                return Some(token);
            }
        }
        None
    }

    /// Emit a row to BigQuery and return an SDK response.
    fn emit_row(&self, table: &str, row: &Value, label: &str) -> Value {
        match self.insert_row(table, row) {
            Ok(()) => {
                eprintln!("[bigquery-sync] emitted: {label}");
                ok()
            }
            Err(e) => {
                eprintln!("[bigquery-sync] emit failed ({label}): {e}");
                ok() // Don't error — telemetry failure shouldn't break devdock
            }
        }
    }

    /// Insert a row into BigQuery using the REST API.
    fn insert_row(&self, table: &str, row: &Value) -> Result<(), String> {
        let token = self
            .get_access_token()
            .ok_or_else(|| "Failed to get access token".to_string())?;

        let url = format!(
            "https://bigquery.googleapis.com/bigquery/v2/projects/{}/datasets/{}/tables/{}/insertAll",
            self.project_id, self.dataset, table
        );

        let body = serde_json::json!({
            "rows": [{"json": row}]
        });

        let client = reqwest::blocking::Client::new();
        let response = client
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .map_err(|e| format!("HTTP error: {e}"))?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            Err(format!("BigQuery API error {status}: {body}"))
        }
    }
}

impl Plugin for BigQuerySync {
    fn name(&self) -> &str {
        "bigquery-sync"
    }

    fn initialize(&mut self, config: Value) {
        // Use cloud_project (GCP project ID) for BigQuery, not project (app name)
        self.project_id = config["cloud_project"]
            .as_str()
            .or_else(|| config["project"].as_str())
            .unwrap_or("")
            .to_string();
        self.dataset = config["dataset"]
            .as_str()
            .unwrap_or("devdock")
            .to_string();

        eprintln!(
            "[bigquery-sync] initialized: project={}, dataset={}",
            self.project_id, self.dataset
        );
    }

    fn handle(&self, method: &str, params: &Value) -> Value {
        match method {
            // Lifecycle hooks
            "after_tilt" => {
                let row = serde_json::json!({
                    "session_id": params["session_id"],
                    "profile": params["profile_name"],
                    "event_type": "session_start",
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                });
                self.emit_row("telemetry_events", &row, "after_tilt")
            }
            "after_ready" => {
                let row = serde_json::json!({
                    "session_id": params["session_id"],
                    "profile": params["profile_name"],
                    "resource_count": params["resource_count"],
                    "startup_duration_secs": params["startup_duration_secs"],
                    "event_type": "bootstrap_complete",
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                });
                self.emit_row("telemetry_events", &row, "after_ready")
            }
            "before_stop" => {
                let row = serde_json::json!({
                    "session_id": params["session_id"],
                    "duration_secs": params["duration_secs"],
                    "event_type": "session_end",
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                });
                self.emit_row("telemetry_events", &row, "before_stop")
            }
            // Legacy emit method (backward compatibility)
            "emit" => {
                let event = &params["event"];
                let event_type = event["type"].as_str().unwrap_or("unknown");
                let row = serde_json::json!({
                    "session_id": event["session_id"],
                    "event_type": event_type,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                });
                self.emit_row("telemetry_events", &row, event_type)
            }
            "flush" => ok(),
            _ => ok(), // Ignore unhandled hooks gracefully
        }
    }

    fn shutdown(&self) {
        eprintln!("[bigquery-sync] shutdown");
    }
}

fn main() {
    devdock_plugin_sdk::run::<BigQuerySync>();
}
