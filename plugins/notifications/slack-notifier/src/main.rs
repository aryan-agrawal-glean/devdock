//! DevDock Slack Notifier Plugin
//!
//! Sends notifications to any Slack workspace. Config from devdock.toml:
//!
//! Lifecycle hooks:
//! - `after_ready` — notify when DevDock is ready
//! - `on_error` — notify on unrecoverable errors
//!
//! Config:
//!   [diagnostics]
//!   notifier = "external"
//!   plugin = "devdock-plugin-slack-notifier"
//!   slack_webhook_url = "https://hooks.slack.com/services/YOUR/WEBHOOK/URL"
//!   slack_channel = "#dev-notifications"

use devdock_plugin_sdk::{error, ok, Plugin};
use serde_json::Value;

#[derive(Default)]
struct SlackNotifier {
    webhook_url: String,
    channel: String,
}

impl SlackNotifier {
    fn send_and_ok(&self, text: &str) -> Value {
        match self.send_message(text) {
            Ok(()) => ok(),
            Err(e) => {
                eprintln!("[slack-notifier] send failed: {e}");
                ok() // Don't error — notification failure shouldn't break devdock
            }
        }
    }

    fn send_message(&self, text: &str) -> Result<(), String> {
        if self.webhook_url.is_empty() {
            return Err("Slack webhook URL not configured".into());
        }

        let payload = if self.channel.is_empty() {
            serde_json::json!({"text": text})
        } else {
            serde_json::json!({"text": text, "channel": &self.channel})
        };

        let client = reqwest::blocking::Client::new();
        let response = client
            .post(&self.webhook_url)
            .json(&payload)
            .send()
            .map_err(|e| format!("HTTP error: {e}"))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("Slack API error: {}", response.status()))
        }
    }
}

impl Plugin for SlackNotifier {
    fn name(&self) -> &str {
        "slack-notifier"
    }

    fn initialize(&mut self, config: Value) {
        self.webhook_url = config["slack_webhook_url"]
            .as_str()
            .unwrap_or("")
            .to_string();
        self.channel = config["slack_channel"]
            .as_str()
            .unwrap_or("")
            .to_string();

        // Also try env var for webhook
        if self.webhook_url.is_empty() {
            if let Some(env_var) = config["slack_webhook_env"].as_str() {
                self.webhook_url = std::env::var(env_var).unwrap_or_default();
            }
        }

        eprintln!("[slack-notifier] initialized: channel={}", self.channel);
    }

    fn handle(&self, method: &str, params: &Value) -> Value {
        match method {
            // Lifecycle hooks
            "after_ready" => {
                let profile = params["profile_name"].as_str().unwrap_or("unknown");
                let duration = params["startup_duration_secs"].as_f64().unwrap_or(0.0);
                let count = params["resource_count"].as_u64().unwrap_or(0);
                let msg = format!(
                    ":white_check_mark: DevDock ready — profile `{profile}`, {count} resources in {duration:.0}s"
                );
                self.send_and_ok(&msg)
            }
            "on_error" => {
                let error_msg = params["error"].as_str().unwrap_or("Unknown error");
                let state = params["state"].as_str().unwrap_or("unknown");
                let msg = format!(":x: *DevDock Error* (state: {state}): {error_msg}");
                self.send_and_ok(&msg)
            }
            // Legacy methods
            "notify" => {
                let text = params["text"].as_str().unwrap_or("");
                match self.send_message(text) {
                    Ok(()) => ok(),
                    Err(e) => error(&e),
                }
            }
            _ => ok(), // Ignore unhandled hooks gracefully
        }
    }
}

fn main() {
    devdock_plugin_sdk::run::<SlackNotifier>();
}
