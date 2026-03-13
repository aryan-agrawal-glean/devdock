//! DevDock Glean Config Generator Plugin
//!
//! Generates Glean-specific configuration files (devdock.ini and temp_profile.yaml)
//! during the `pre_tilt` lifecycle hook.
//!
//! This plugin contains all the Glean-specific config generation logic that was
//! previously in the `devdock-config` crate:
//! - INI generation with service-specific configs
//! - Platform auth flows (GKE/EKS/AKS)
//! - SQL import patterns (Cloud SQL Proxy, bastion, hydration)
//! - Dynamic config loading from GCS/S3
//! - Profile generation with bastion/namespace
//!
//! ## Hook: pre_tilt
//!
//! Input:
//! ```json
//! {
//!     "method": "pre_tilt",
//!     "params": {
//!         "profile": { ... },
//!         "runtime_dir": "/path/to/runtime_generated",
//!         "workspace_root": "/path/to/workspace"
//!     }
//! }
//! ```
//!
//! The plugin:
//! 1. Parses the profile from params
//! 2. Runs the full ConfigGenerator pipeline (INI + profile + watch files)
//! 3. Writes devdock.ini and temp_profile.yaml to the workspace
//!
//! ## Registration
//!
//! In `.devdock/devdock.toml`:
//! ```toml
//! [plugins.hooks]
//! pre_tilt = ["devdock-plugin-config-generator"]
//! ```

mod constants;
mod gcs_storage;
mod generator;
mod ini_generator;
mod ini_parser;
mod opensearch;
mod opensearch_import;
mod platform;
mod profile_generator;
mod s3_storage;
mod sql;
mod sql_import;

use devdock_plugin_sdk::{error, ok, ok_with_data, Plugin};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Default)]
struct ConfigGeneratorPlugin {
    workspace_root: Option<PathBuf>,
    project: String,
}

impl Plugin for ConfigGeneratorPlugin {
    fn name(&self) -> &str {
        "config-generator"
    }

    fn initialize(&mut self, config: Value) {
        self.project = config["project"].as_str().unwrap_or("").to_string();

        if let Some(root) = config["workspace_root"].as_str() {
            self.workspace_root = Some(PathBuf::from(root));
        }

        eprintln!(
            "[config-generator] initialized: project={}, workspace_root={:?}",
            self.project, self.workspace_root
        );
    }

    fn handle(&self, method: &str, params: &Value) -> Value {
        match method {
            "pre_tilt" => self.handle_pre_tilt(params),
            "pre_start" | "post_start" | "on_stop" | "on_report" => {
                // No-op for hooks we don't care about
                ok()
            }
            _ => ok(),
        }
    }
}

impl ConfigGeneratorPlugin {
    fn handle_pre_tilt(&self, params: &Value) -> Value {
        let workspace_root = if let Some(root) = params["params"]["workspace_root"].as_str() {
            PathBuf::from(root)
        } else if let Some(ref root) = self.workspace_root {
            root.clone()
        } else {
            return error("workspace_root not provided");
        };

        // Parse profile from params
        let profile_value = &params["params"]["profile"];
        let profile: devdock_core::Profile = match serde_json::from_value(profile_value.clone()) {
            Ok(p) => p,
            Err(e) => return error(&format!("Failed to parse profile: {e}")),
        };

        // Build a tokio runtime for async operations
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => return error(&format!("Failed to create runtime: {e}")),
        };

        // Run the config generator (now local to this plugin crate)
        let result = rt.block_on(async {
            use crate::generator::{ConfigGenerator, WatchStrategy};

            let generator = ConfigGenerator::new(workspace_root)
                .with_watch_strategy(WatchStrategy::Static);

            generator.generate(&profile, None, None).await
        });

        match result {
            Ok(()) => {
                eprintln!("[config-generator] Generated configs successfully");
                ok_with_data(serde_json::json!({
                    "files": ["devdock.ini", "temp_profile.yaml"]
                }))
            }
            Err(e) => {
                eprintln!("[config-generator] Config generation failed: {e}");
                error(&format!("Config generation failed: {e}"))
            }
        }
    }
}

fn main() {
    devdock_plugin_sdk::run::<ConfigGeneratorPlugin>();
}
