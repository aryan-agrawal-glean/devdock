//! Slack notification for bug reports.

use crate::{DiagnosticsError, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;
use tracing::{info, warn};

const SLACK_CHANNEL: &str = "help-devdock";

#[derive(Debug, Serialize)]
struct SlackMessage {
    channel: String,
    blocks: Vec<SlackBlock>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SlackBlock {
    Header {
        text: SlackText,
    },
    Section {
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<SlackText>,
        #[serde(skip_serializing_if = "Option::is_none")]
        fields: Option<Vec<SlackText>>,
    },
    Actions {
        elements: Vec<SlackElement>,
    },
}

#[derive(Debug, Serialize)]
struct SlackText {
    #[serde(rename = "type")]
    text_type: String,
    text: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SlackElement {
    Button { text: SlackText, url: String },
}

#[derive(Debug, Deserialize)]
struct SlackResponse {
    ok: bool,
    error: Option<String>,
}

/// Get Slack bot token from Google Cloud Secret Manager.
pub fn get_slack_bot_token() -> Result<String> {
    let output = Command::new("gcloud")
        .args([
            "--project",
            "dev-sandbox-334901",
            "secrets",
            "versions",
            "access",
            "1",
            "--secret=GLEAN_AUTOMATION_SLACK_BOT_TOKEN",
        ])
        .output()
        .map_err(|e| DiagnosticsError::Slack(format!("Failed to run gcloud command: {}", e)))?;

    if output.status.success() {
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(token)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(DiagnosticsError::Slack(format!("Failed to fetch Slack bot token: {}", stderr)))
    }
}

/// Post a message to Slack when a bug report is created.
pub async fn post_to_slack(jira_key: &str, summary: &str, reporter_email: &str) -> Result<()> {
    info!("Posting to Slack...");

    let bot_token = match get_slack_bot_token() {
        Ok(token) => token,
        Err(e) => {
            warn!("Failed to get Slack bot token, skipping notification: {}", e);
            return Ok(());
        }
    };

    let jira_url = format!("https://askscio.atlassian.net/browse/{}", jira_key);
    let reporter_name = reporter_email.split('@').next().unwrap_or(reporter_email);

    let message = SlackMessage {
        channel: SLACK_CHANNEL.to_string(),
        blocks: vec![
            SlackBlock::Header {
                text: SlackText {
                    text_type: "plain_text".to_string(),
                    text: "🐛 New DevDock Bug Report".to_string(),
                },
            },
            SlackBlock::Section {
                text: None,
                fields: Some(vec![
                    SlackText {
                        text_type: "mrkdwn".to_string(),
                        text: format!("*Issue:* <{}|{}>", jira_url, jira_key),
                    },
                    SlackText {
                        text_type: "mrkdwn".to_string(),
                        text: format!("*Reporter:* {}", reporter_name),
                    },
                ]),
            },
            SlackBlock::Section {
                text: Some(SlackText {
                    text_type: "mrkdwn".to_string(),
                    text: format!("*Summary:* {}", summary),
                }),
                fields: None,
            },
            SlackBlock::Actions {
                elements: vec![SlackElement::Button {
                    text: SlackText {
                        text_type: "plain_text".to_string(),
                        text: "View in Jira".to_string(),
                    },
                    url: jira_url.clone(),
                }],
            },
        ],
    };

    let client = reqwest::Client::new();
    let response = client
        .post("https://slack.com/api/chat.postMessage")
        .header("Authorization", format!("Bearer {}", bot_token))
        .header("Content-Type", "application/json")
        .json(&message)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await?;
        return Err(DiagnosticsError::Slack(format!(
            "Failed to post to Slack (status {}): {}",
            status, body
        )));
    }

    let slack_response: SlackResponse = response.json().await?;
    if slack_response.ok {
        info!("Successfully posted bug report to Slack");
        Ok(())
    } else {
        let error = slack_response.error.unwrap_or_else(|| "Unknown error".to_string());
        Err(DiagnosticsError::Slack(format!("Slack API returned error: {}", error)))
    }
}
