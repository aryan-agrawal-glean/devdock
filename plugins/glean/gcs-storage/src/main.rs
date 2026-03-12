//! DevDock GCS Storage Plugin
//!
//! Reads configuration files from Google Cloud Storage buckets.
//! This is Glean's infra pattern — generic repos use local config.
//!
//! Config:
//!   [cloud]
//!   storage = "external"
//!   plugin = "devdock-plugin-gcs-storage"
//!   project = "your-gcp-project"
//!   config_bucket = "config-{project}"

use devdock_plugin_sdk::{error, ok, ok_with_data, Plugin};
use serde_json::Value;
use std::process::Command;

#[derive(Default)]
struct GcsStorage {
    project: String,
    config_bucket: String,
}

impl Plugin for GcsStorage {
    fn name(&self) -> &str {
        "gcs-storage"
    }

    fn initialize(&mut self, config: Value) {
        self.project = config["project"].as_str().unwrap_or("").to_string();
        self.config_bucket = config["config_bucket"]
            .as_str()
            .unwrap_or("")
            .to_string();

        // Default bucket pattern
        if self.config_bucket.is_empty() && !self.project.is_empty() {
            self.config_bucket = format!("config-{}", self.project);
        }

        eprintln!(
            "[gcs-storage] initialized: project={}, bucket={}",
            self.project, self.config_bucket
        );
    }

    fn handle(&self, method: &str, params: &Value) -> Value {
        match method {
            "fetch" => {
                let key = params["key"].as_str().unwrap_or("");
                let uri = format!("gs://{}/{}", self.config_bucket, key);

                match Command::new("gsutil").args(["cat", &uri]).output() {
                    Ok(output) if output.status.success() => {
                        let content = String::from_utf8_lossy(&output.stdout).to_string();
                        ok_with_data(serde_json::json!({"content": content}))
                    }
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        error(&format!("gsutil failed: {stderr}"))
                    }
                    Err(e) => error(&format!("gsutil error: {e}")),
                }
            }
            "exists" => {
                let key = params["key"].as_str().unwrap_or("");
                let uri = format!("gs://{}/{}", self.config_bucket, key);

                let exists = Command::new("gsutil")
                    .args(["stat", &uri])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);

                ok_with_data(serde_json::json!({"exists": exists}))
            }
            "list" => {
                let prefix = params["prefix"].as_str().unwrap_or("");
                let uri = format!("gs://{}/{}", self.config_bucket, prefix);

                match Command::new("gsutil").args(["ls", &uri]).output() {
                    Ok(output) if output.status.success() => {
                        let files: Vec<&str> = String::from_utf8_lossy(&output.stdout)
                            .lines()
                            .collect();
                        ok_with_data(serde_json::json!({"files": files}))
                    }
                    _ => ok_with_data(serde_json::json!({"files": []})),
                }
            }
            _ => error(&format!("Unknown method: {method}")),
        }
    }
}

fn main() {
    devdock_plugin_sdk::run::<GcsStorage>();
}
