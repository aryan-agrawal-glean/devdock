//! DevDock S3 Storage Plugin
//!
//! Reads configuration files from AWS S3 buckets.
//!
//! Config:
//!   [cloud]
//!   storage = "external"
//!   plugin = "devdock-plugin-s3-storage"
//!   account_id = "123456789012"
//!   config_bucket = "config-{account_id}"
//!   region = "us-east-1"

use devdock_plugin_sdk::{error, ok, ok_with_data, Plugin};
use serde_json::Value;
use std::process::Command;

#[derive(Default)]
struct S3Storage {
    config_bucket: String,
    region: String,
}

impl Plugin for S3Storage {
    fn name(&self) -> &str {
        "s3-storage"
    }

    fn initialize(&mut self, config: Value) {
        let account_id = config["account_id"].as_str().unwrap_or("");
        self.config_bucket = config["config_bucket"]
            .as_str()
            .unwrap_or("")
            .to_string();
        self.region = config["region"]
            .as_str()
            .unwrap_or("us-east-1")
            .to_string();

        if self.config_bucket.is_empty() && !account_id.is_empty() {
            self.config_bucket = format!("config-{account_id}");
        }

        eprintln!(
            "[s3-storage] initialized: bucket={}, region={}",
            self.config_bucket, self.region
        );
    }

    fn handle(&self, method: &str, params: &Value) -> Value {
        match method {
            "fetch" => {
                let key = params["key"].as_str().unwrap_or("");
                let uri = format!("s3://{}/{}", self.config_bucket, key);

                match Command::new("aws")
                    .args(["s3", "cp", &uri, "-", "--region", &self.region])
                    .output()
                {
                    Ok(output) if output.status.success() => {
                        let content = String::from_utf8_lossy(&output.stdout).to_string();
                        ok_with_data(serde_json::json!({"content": content}))
                    }
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        error(&format!("aws s3 failed: {stderr}"))
                    }
                    Err(e) => error(&format!("aws cli error: {e}")),
                }
            }
            "exists" => {
                let key = params["key"].as_str().unwrap_or("");
                let uri = format!("s3://{}/{}", self.config_bucket, key);

                let exists = Command::new("aws")
                    .args(["s3", "ls", &uri, "--region", &self.region])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);

                ok_with_data(serde_json::json!({"exists": exists}))
            }
            _ => error(&format!("Unknown method: {method}")),
        }
    }
}

fn main() {
    devdock_plugin_sdk::run::<S3Storage>();
}
