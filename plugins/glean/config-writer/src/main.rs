//! DevDock Glean Config Writer Plugin
//!
//! Generates devdock.ini and enriches configuration files after initial
//! config generation. Extracted from the monolithic config-generator.
//!
//! Lifecycle hook: `after_config_generated`

use devdock_plugin_sdk::{ok, Plugin};
use serde_json::Value;

#[derive(Default)]
struct ConfigWriter {
    project: String,
    workspace_root: String,
}

impl Plugin for ConfigWriter {
    fn name(&self) -> &str {
        "config-writer"
    }

    fn initialize(&mut self, config: Value) {
        self.project = config["project"].as_str().unwrap_or("").to_string();
        self.workspace_root = config["workspace_root"].as_str().unwrap_or("").to_string();
        eprintln!("[config-writer] initialized: project={}", self.project);
    }

    fn handle(&self, method: &str, params: &Value) -> Value {
        match method {
            "after_config_generated" => {
                let runtime_dir = params["runtime_dir"].as_str().unwrap_or("");
                eprintln!("[config-writer] writing configs to {runtime_dir}");

                // TODO: Implement INI generation from the original
                // config-generator's generator.rs, ini_generator.rs,
                // profile_generator.rs, and constants.rs modules
                ok()
            }
            _ => ok(),
        }
    }
}

fn main() {
    devdock_plugin_sdk::run::<ConfigWriter>();
}
