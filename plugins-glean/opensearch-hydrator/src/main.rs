//! DevDock Glean OpenSearch Hydrator Plugin
//!
//! Hydrates OpenSearch indexes after the OpenSearch infrastructure resource starts.
//! Extracted from the monolithic config-generator.
//!
//! Lifecycle hook: `after_resource_start` (resource_name: "opensearch")

use devdock_plugin_sdk::{ok, Plugin};
use serde_json::Value;

#[derive(Default)]
struct OpensearchHydrator {
    workspace_root: String,
}

impl Plugin for OpensearchHydrator {
    fn name(&self) -> &str {
        "opensearch-hydrator"
    }

    fn initialize(&mut self, config: Value) {
        self.workspace_root = config["workspace_root"].as_str().unwrap_or("").to_string();
        eprintln!("[opensearch-hydrator] initialized");
    }

    fn handle(&self, method: &str, params: &Value) -> Value {
        match method {
            "after_resource_start" => {
                let resource = params["resource_name"].as_str().unwrap_or("");
                let kind = params["resource_kind"].as_str().unwrap_or("");

                if resource == "opensearch" && kind == "infra" {
                    eprintln!("[opensearch-hydrator] reindexing OpenSearch...");

                    // TODO: Implement OpenSearch reindexing from the original
                    // config-generator's opensearch.rs and opensearch_import.rs modules
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
    devdock_plugin_sdk::run::<OpensearchHydrator>();
}
