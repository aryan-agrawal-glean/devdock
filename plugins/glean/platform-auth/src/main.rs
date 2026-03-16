//! DevDock Glean Platform Auth Plugin
//!
//! Handles platform authentication before infrastructure setup.
//! Extracted from the monolithic config-generator plugin.
//!
//! Lifecycle hook: `before_infra_ready`

use devdock_plugin_sdk::{ok, error, Plugin};
use serde_json::Value;

#[derive(Default)]
struct PlatformAuth {
    platform: String,
    project: String,
}

impl Plugin for PlatformAuth {
    fn name(&self) -> &str {
        "platform-auth"
    }

    fn initialize(&mut self, config: Value) {
        self.platform = config["platform"].as_str().unwrap_or("none").to_string();
        self.project = config["project"].as_str().unwrap_or("").to_string();
        eprintln!(
            "[platform-auth] initialized: platform={}, project={}",
            self.platform, self.project
        );
    }

    fn handle(&self, method: &str, _params: &Value) -> Value {
        match method {
            "before_infra_ready" => {
                eprintln!("[platform-auth] authenticating platform: {}", self.platform);

                match self.platform.as_str() {
                    "google" => {
                        // Verify gcloud auth
                        let output = std::process::Command::new("gcloud")
                            .args(["auth", "application-default", "print-access-token"])
                            .output();

                        match output {
                            Ok(o) if o.status.success() => {
                                eprintln!("[platform-auth] gcloud auth verified");
                                ok()
                            }
                            _ => {
                                eprintln!("[platform-auth] gcloud auth failed — run: gcloud auth application-default login");
                                error("gcloud authentication required")
                            }
                        }
                    }
                    "aws" => {
                        // Verify AWS credentials
                        let output = std::process::Command::new("aws")
                            .args(["sts", "get-caller-identity"])
                            .output();

                        match output {
                            Ok(o) if o.status.success() => {
                                eprintln!("[platform-auth] AWS auth verified");
                                ok()
                            }
                            _ => {
                                eprintln!("[platform-auth] AWS auth failed — run: aws sso login");
                                error("AWS authentication required")
                            }
                        }
                    }
                    _ => ok(), // No auth needed for "none" or unknown platforms
                }
            }
            _ => ok(),
        }
    }
}

fn main() {
    devdock_plugin_sdk::run::<PlatformAuth>();
}
