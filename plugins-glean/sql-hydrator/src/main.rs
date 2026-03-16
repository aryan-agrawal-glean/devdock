//! DevDock Glean SQL Hydrator Plugin
//!
//! Hydrates SQL databases after the SQL infrastructure resource starts.
//! Extracted from the monolithic config-generator.
//!
//! Lifecycle hook: `after_resource_start` (resource_name: "sql")

use devdock_plugin_sdk::{ok, Plugin};
use serde_json::Value;

#[derive(Default)]
struct SqlHydrator {
    workspace_root: String,
}

impl Plugin for SqlHydrator {
    fn name(&self) -> &str {
        "sql-hydrator"
    }

    fn initialize(&mut self, config: Value) {
        self.workspace_root = config["workspace_root"].as_str().unwrap_or("").to_string();
        eprintln!("[sql-hydrator] initialized");
    }

    fn handle(&self, method: &str, params: &Value) -> Value {
        match method {
            "after_resource_start" => {
                let resource = params["resource_name"].as_str().unwrap_or("");
                let kind = params["resource_kind"].as_str().unwrap_or("");

                if resource == "sql" && kind == "infra" {
                    eprintln!("[sql-hydrator] hydrating SQL database...");

                    // TODO: Implement SQL hydration from the original
                    // config-generator's sql.rs and sql_import.rs modules
                    ok()
                } else {
                    ok() // Not our resource
                }
            }
            _ => ok(),
        }
    }
}

fn main() {
    devdock_plugin_sdk::run::<SqlHydrator>();
}
