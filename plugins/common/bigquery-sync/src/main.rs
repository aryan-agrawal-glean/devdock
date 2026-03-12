//! DevDock BigQuery Sync Plugin
//!
//! Syncs telemetry data (collected by devdock core) to Google BigQuery.
//! All config (project, dataset) comes from devdock.toml via initialize().
//!
//! Install: cargo install --path plugins/common/bigquery-sync
//! Config:
//!   [telemetry]
//!   backend = "external"
//!   plugin = "devdock-plugin-bigquery-sync"
//!   project = "your-gcp-project"
//!   dataset = "devdock"

use devdock_plugin_sdk::{ok, error, Plugin};
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
        self.project_id = config["project"]
            .as_str()
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
            "emit" => {
                let event = &params["event"];
                let event_type = event["type"].as_str().unwrap_or("unknown");

                // Build BigQuery row from telemetry event
                let row = match event_type {
                    "SessionStart" => serde_json::json!({
                        "session_id": event["session_id"],
                        "profile": event["profile"],
                        "mode": event["mode"],
                        "event_type": "session_start",
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    }),
                    "SessionEnd" => serde_json::json!({
                        "session_id": event["session_id"],
                        "duration_secs": event["duration_secs"],
                        "success": event["success"],
                        "event_type": "session_end",
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    }),
                    "BootstrapComplete" => serde_json::json!({
                        "session_id": event["session_id"],
                        "bootstrap_time_ms": event["bootstrap_time_ms"],
                        "event_type": "bootstrap_complete",
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    }),
                    "Error" => serde_json::json!({
                        "session_id": event["session_id"],
                        "message": event["message"],
                        "event_type": "error",
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    }),
                    _ => {
                        return error(&format!("Unknown event type: {event_type}"));
                    }
                };

                match self.insert_row("telemetry_events", &row) {
                    Ok(()) => {
                        eprintln!("[bigquery-sync] emitted: {event_type}");
                        ok()
                    }
                    Err(e) => {
                        eprintln!("[bigquery-sync] emit failed: {e}");
                        // Don't error — telemetry failure shouldn't break devdock
                        ok()
                    }
                }
            }
            "flush" => ok(),
            _ => error(&format!("Unknown method: {method}")),
        }
    }

    fn shutdown(&self) {
        eprintln!("[bigquery-sync] shutdown");
    }
}

fn main() {
    devdock_plugin_sdk::run::<BigQuerySync>();
}
