//! DevDock Glean Config Fetcher Plugin
//!
//! Fetches remote configuration files from cloud storage (GCS/S3)
//! before config generation. Extracted from the monolithic config-generator.
//!
//! Lifecycle hook: `before_config_generated`

use devdock_plugin_sdk::{ok, Plugin};
use serde_json::Value;

#[derive(Default)]
struct ConfigFetcher {
    platform: String,
    project: String,
    workspace_root: String,
}

impl Plugin for ConfigFetcher {
    fn name(&self) -> &str {
        "config-fetcher"
    }

    fn initialize(&mut self, config: Value) {
        self.platform = config["platform"].as_str().unwrap_or("none").to_string();
        self.project = config["project"].as_str().unwrap_or("").to_string();
        self.workspace_root = config["workspace_root"].as_str().unwrap_or("").to_string();
        eprintln!(
            "[config-fetcher] initialized: platform={}, project={}",
            self.platform, self.project
        );
    }

    fn handle(&self, method: &str, params: &Value) -> Value {
        match method {
            "before_config_generated" => {
                let runtime_dir = params["runtime_dir"].as_str().unwrap_or("");
                eprintln!("[config-fetcher] fetching configs to {runtime_dir}");

                // Ensure runtime directory exists
                if !runtime_dir.is_empty() {
                    std::fs::create_dir_all(runtime_dir).ok();
                }

                // TODO: Implement GCS/S3 config fetching from the original
                // config-generator's gcs_storage.rs and s3_storage.rs modules
                ok()
            }
            _ => ok(),
        }
    }
}

fn main() {
    devdock_plugin_sdk::run::<ConfigFetcher>();
}
