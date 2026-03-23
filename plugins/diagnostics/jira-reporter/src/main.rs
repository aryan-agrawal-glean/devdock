//! DevDock Jira Reporter Plugin
//!
//! Reports bugs to any Jira instance. All config comes from devdock.toml:
//!
//! Lifecycle hooks:
//! - `on_error` — auto-create ticket for unrecoverable errors
//! - `on_report` — create ticket from `devdock report` command
//!
//! Config:
//!   [diagnostics]
//!   reporter = "external"
//!   plugin = "devdock-plugin-jira-reporter"
//!   jira_url = "https://your-org.atlassian.net"
//!   jira_project = "ENG"
//!   jira_email = "bot@your-org.com"
//!   jira_token_env = "JIRA_API_TOKEN"    # or jira_token_secret for GCP Secret Manager

use devdock_plugin_sdk::{error, ok, ok_with_data, Plugin};
use serde_json::Value;

#[derive(Default)]
struct JiraReporter {
    jira_url: String,
    project_key: String,
    email: String,
    api_token: String,
}

impl JiraReporter {
    fn get_token(config: &Value) -> String {
        // Try env var first
        if let Some(env_var) = config["jira_token_env"].as_str() {
            if let Ok(token) = std::env::var(env_var) {
                return token;
            }
        }

        // Try GCP Secret Manager
        if let Some(secret) = config["jira_token_secret"].as_str() {
            let project = config["secret_project"].as_str().unwrap_or("");
            if let Ok(output) = std::process::Command::new("gcloud")
                .args([
                    "secrets", "versions", "access", "latest",
                    &format!("--secret={secret}"),
                    &format!("--project={project}"),
                ])
                .output()
            {
                if output.status.success() {
                    return String::from_utf8_lossy(&output.stdout).trim().to_string();
                }
            }
        }

        String::new()
    }

    fn create_ticket(&self, title: &str, body: &str) -> Result<String, String> {
        if self.jira_url.is_empty() || self.api_token.is_empty() {
            return Err("Jira not configured (missing URL or token)".into());
        }

        let url = format!("{}/rest/api/3/issue", self.jira_url);

        let payload = serde_json::json!({
            "fields": {
                "project": {"key": &self.project_key},
                "summary": title,
                "description": {
                    "type": "doc",
                    "version": 1,
                    "content": [{
                        "type": "paragraph",
                        "content": [{"type": "text", "text": body}]
                    }]
                },
                "issuetype": {"name": "Bug"}
            }
        });

        let auth = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("{}:{}", self.email, self.api_token),
        );

        let client = reqwest::blocking::Client::new();
        let response = client
            .post(&url)
            .header("Authorization", format!("Basic {auth}"))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .map_err(|e| format!("HTTP error: {e}"))?;

        if response.status().is_success() {
            let result: Value = response.json().map_err(|e| format!("JSON error: {e}"))?;
            let key = result["key"].as_str().unwrap_or("unknown");
            let ticket_url = format!("{}/browse/{key}", self.jira_url);
            Ok(ticket_url)
        } else {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            Err(format!("Jira API error {status}: {body}"))
        }
    }
}

impl Plugin for JiraReporter {
    fn name(&self) -> &str {
        "jira-reporter"
    }

    fn initialize(&mut self, config: Value) {
        self.jira_url = config["jira_url"].as_str().unwrap_or("").to_string();
        self.project_key = config["jira_project"].as_str().unwrap_or("ENG").to_string();
        self.email = config["jira_email"].as_str().unwrap_or("").to_string();
        self.api_token = Self::get_token(&config);

        eprintln!(
            "[jira-reporter] initialized: url={}, project={}",
            self.jira_url, self.project_key
        );
    }

    fn handle(&self, method: &str, params: &Value) -> Value {
        match method {
            // Lifecycle hooks
            "on_error" => {
                let error_msg = params["error"].as_str().unwrap_or("Unknown error");
                let state = params["state"].as_str().unwrap_or("unknown");
                let session = params["session_id"].as_str().unwrap_or("");
                let title = format!("DevDock Error (state: {state})");
                let body = format!("Session: {session}\nState: {state}\nError: {error_msg}");

                match self.create_ticket(&title, &body) {
                    Ok(url) => {
                        eprintln!("[jira-reporter] on_error ticket: {url}");
                        ok_with_data(serde_json::json!({"url": url}))
                    }
                    Err(e) => {
                        eprintln!("[jira-reporter] on_error failed: {e}");
                        ok() // Don't cascade errors
                    }
                }
            }
            "on_report" | "report" => {
                let title = params["title"].as_str().unwrap_or("DevDock Bug Report");
                let body = params["body"].as_str().unwrap_or("");

                match self.create_ticket(title, body) {
                    Ok(url) => {
                        eprintln!("[jira-reporter] created: {url}");
                        ok_with_data(serde_json::json!({"url": url}))
                    }
                    Err(e) => {
                        eprintln!("[jira-reporter] failed: {e}");
                        error(&e)
                    }
                }
            }
            _ => ok(), // Ignore unhandled hooks gracefully
        }
    }
}

fn main() {
    devdock_plugin_sdk::run::<JiraReporter>();
}
