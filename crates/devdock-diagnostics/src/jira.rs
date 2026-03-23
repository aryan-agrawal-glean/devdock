//! Jira integration for bug report submission.

use crate::{system::SystemInfo, DiagnosticsError, Result};
use base64::Engine;
use reqwest::multipart;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use tracing::{info, warn};

/// Jira URL — configured via DEVDOCK_JIRA_URL env var or devdock.toml diagnostics.jira_url
fn jira_url() -> String {
    std::env::var("DEVDOCK_JIRA_URL").unwrap_or_default()
}

/// Get the Jira URL (public, for checking if configured)
pub fn get_jira_url() -> String {
    jira_url()
}

fn parent_issue() -> String {
    std::env::var("DEVDOCK_JIRA_PARENT_ISSUE").unwrap_or_default()
}
fn jira_email() -> String {
    std::env::var("DEVDOCK_JIRA_EMAIL").unwrap_or_default()
}

#[derive(Debug, Serialize)]
struct JiraIssueFields {
    project: JiraProject,
    summary: String,
    description: JiraDescription,
    issuetype: JiraIssueType,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<JiraParent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reporter: Option<JiraReporter>,
}

#[derive(Debug, Serialize)]
pub struct JiraDescription {
    version: u8,
    #[serde(rename = "type")]
    doc_type: String,
    content: Vec<JiraContent>,
}

#[derive(Debug, Serialize)]
pub struct JiraContent {
    #[serde(rename = "type")]
    content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    attrs: Option<JiraAttrs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Vec<JiraText>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct JiraAttrs {
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct JiraText {
    #[serde(rename = "type")]
    text_type: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct JiraProject {
    key: String,
}

#[derive(Debug, Serialize)]
struct JiraIssueType {
    name: String,
}

#[derive(Debug, Serialize)]
struct JiraParent {
    key: String,
}

#[derive(Debug, Serialize)]
struct JiraReporter {
    #[serde(rename = "accountId")]
    account_id: String,
}

#[derive(Debug, Serialize)]
struct JiraCreateIssue {
    fields: JiraIssueFields,
}

#[derive(Debug, Deserialize)]
struct JiraIssueResponse {
    key: String,
}

#[derive(Debug, Deserialize)]
struct JiraUser {
    #[serde(rename = "accountId")]
    account_id: String,
    #[serde(rename = "emailAddress")]
    email_address: Option<String>,
}

/// Get Jira API token from Google Cloud Secret Manager.
pub fn get_jira_token() -> Result<String> {
    let secret_project = std::env::var("DEVDOCK_SECRET_PROJECT").unwrap_or_default();
    let secret_name = std::env::var("DEVDOCK_JIRA_SECRET").map_err(|_| {
        DiagnosticsError::Jira(
            "DEVDOCK_JIRA_SECRET not set — configure diagnostics.jira_secret in devdock.toml"
                .into(),
        )
    })?;
    if secret_project.is_empty() {
        return Err(DiagnosticsError::Jira(
            "DEVDOCK_SECRET_PROJECT not set — configure diagnostics.secret_project in devdock.toml"
                .into(),
        ));
    }
    let output = Command::new("gcloud")
        .args([
            "--project",
            &secret_project,
            "secrets",
            "versions",
            "access",
            "latest",
            &format!("--secret={secret_name}"),
        ])
        .output()
        .map_err(|e| DiagnosticsError::Jira(format!("Failed to run gcloud command: {e}")))?;

    if output.status.success() {
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(token)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(DiagnosticsError::Jira(format!("Failed to fetch Jira token: {stderr}")))
    }
}

/// Get reporter email from gcloud config.
pub fn get_reporter_email() -> Result<String> {
    let output = Command::new("gcloud")
        .args(["config", "get-value", "account"])
        .output()
        .map_err(|e| DiagnosticsError::Jira(format!("Failed to run gcloud command: {e}")))?;

    if output.status.success() {
        let email = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !email.is_empty() {
            return Ok(email);
        }
    }

    warn!("Failed to get gcloud account, using oncall user");
    Ok(jira_email())
}

/// Get Jira account ID for a user by their email.
async fn get_user_account_id(
    client: &reqwest::Client,
    email: &str,
    token: &str,
) -> Result<Option<String>> {
    let url = format!("{}/rest/api/3/user/search", jira_url());
    let auth = format!("{}:{token}", jira_email());
    let encoded = base64::engine::general_purpose::STANDARD.encode(auth.as_bytes());

    let response = client
        .get(&url)
        .header("Authorization", format!("Basic {encoded}"))
        .query(&[("query", email)])
        .send()
        .await?;

    if !response.status().is_success() {
        warn!("Failed to search for user: {:?}", response.status());
        return Ok(None);
    }

    let users: Vec<JiraUser> = response.json().await?;
    for user in users {
        if let Some(user_email) = &user.email_address {
            if user_email == email {
                return Ok(Some(user.account_id));
            }
        }
    }

    Ok(None)
}

/// Format the Jira description with all collected information in ADF format.
pub fn format_description(
    description: &str,
    command: &str,
    git_branch: &str,
    system_info: &SystemInfo,
    config_files: &HashMap<String, String>,
) -> JiraDescription {
    let mut content = Vec::new();

    // User Description section
    content.push(JiraContent {
        content_type: "heading".to_string(),
        attrs: Some(JiraAttrs { level: Some(2), language: None }),
        content: Some(vec![JiraText {
            text_type: "text".to_string(),
            text: "User Description".to_string(),
        }]),
        text: None,
    });
    content.push(JiraContent {
        content_type: "paragraph".to_string(),
        attrs: None,
        content: Some(vec![JiraText {
            text_type: "text".to_string(),
            text: description.to_string(),
        }]),
        text: None,
    });

    // Command Run section
    content.push(JiraContent {
        content_type: "heading".to_string(),
        attrs: Some(JiraAttrs { level: Some(2), language: None }),
        content: Some(vec![JiraText {
            text_type: "text".to_string(),
            text: "Command Run".to_string(),
        }]),
        text: None,
    });
    content.push(JiraContent {
        content_type: "codeBlock".to_string(),
        attrs: Some(JiraAttrs { level: None, language: Some("bash".to_string()) }),
        content: Some(vec![JiraText { text_type: "text".to_string(), text: command.to_string() }]),
        text: None,
    });

    // Git Branch section
    content.push(JiraContent {
        content_type: "heading".to_string(),
        attrs: Some(JiraAttrs { level: Some(2), language: None }),
        content: Some(vec![JiraText {
            text_type: "text".to_string(),
            text: "Git Branch".to_string(),
        }]),
        text: None,
    });
    content.push(JiraContent {
        content_type: "codeBlock".to_string(),
        attrs: None,
        content: Some(vec![JiraText {
            text_type: "text".to_string(),
            text: git_branch.to_string(),
        }]),
        text: None,
    });

    // System Information section
    content.push(JiraContent {
        content_type: "heading".to_string(),
        attrs: Some(JiraAttrs { level: Some(2), language: None }),
        content: Some(vec![JiraText {
            text_type: "text".to_string(),
            text: "System Information".to_string(),
        }]),
        text: None,
    });
    content.push(JiraContent {
        content_type: "paragraph".to_string(),
        attrs: None,
        content: Some(vec![JiraText {
            text_type: "text".to_string(),
            text: format!(
                "CPU Usage: {:.1}% | Memory Usage: {:.1}% | Disk Usage: {:.1}%",
                system_info.cpu.percent, system_info.memory.percent, system_info.disk.percent
            ),
        }]),
        text: None,
    });

    // Docker Containers section
    content.push(JiraContent {
        content_type: "heading".to_string(),
        attrs: Some(JiraAttrs { level: Some(2), language: None }),
        content: Some(vec![JiraText {
            text_type: "text".to_string(),
            text: "Docker Containers".to_string(),
        }]),
        text: None,
    });
    content.push(JiraContent {
        content_type: "codeBlock".to_string(),
        attrs: Some(JiraAttrs { level: None, language: Some("json".to_string()) }),
        content: Some(vec![JiraText {
            text_type: "text".to_string(),
            text: serde_json::to_string_pretty(&system_info.docker).unwrap_or_default(),
        }]),
        text: None,
    });

    // Configuration Files section (if any)
    if !config_files.is_empty() {
        content.push(JiraContent {
            content_type: "heading".to_string(),
            attrs: Some(JiraAttrs { level: Some(2), language: None }),
            content: Some(vec![JiraText {
                text_type: "text".to_string(),
                text: "Configuration Files".to_string(),
            }]),
            text: None,
        });
        for (filename, file_content) in config_files {
            content.push(JiraContent {
                content_type: "paragraph".to_string(),
                attrs: None,
                content: Some(vec![JiraText {
                    text_type: "text".to_string(),
                    text: format!("File: {filename}"),
                }]),
                text: None,
            });
            content.push(JiraContent {
                content_type: "codeBlock".to_string(),
                attrs: None,
                content: Some(vec![JiraText {
                    text_type: "text".to_string(),
                    text: file_content.clone(),
                }]),
                text: None,
            });
        }
    }

    // Full System Details section
    content.push(JiraContent {
        content_type: "heading".to_string(),
        attrs: Some(JiraAttrs { level: Some(2), language: None }),
        content: Some(vec![JiraText {
            text_type: "text".to_string(),
            text: "Full System Details".to_string(),
        }]),
        text: None,
    });
    content.push(JiraContent {
        content_type: "codeBlock".to_string(),
        attrs: Some(JiraAttrs { level: None, language: Some("json".to_string()) }),
        content: Some(vec![JiraText {
            text_type: "text".to_string(),
            text: serde_json::to_string_pretty(system_info).unwrap_or_default(),
        }]),
        text: None,
    });

    JiraDescription { version: 1, doc_type: "doc".to_string(), content }
}

/// Create a Jira subtask with the bug report.
pub async fn create_jira_ticket(
    summary: &str,
    description: JiraDescription,
    attachment_path: &Path,
) -> Result<String> {
    info!("Creating Jira ticket...");

    let jira_token = get_jira_token()?;
    let reporter_email = get_reporter_email()?;

    let client = reqwest::Client::new();
    let auth = format!("{}:{jira_token}", jira_email());
    let encoded = base64::engine::general_purpose::STANDARD.encode(auth.as_bytes());

    // Try to get reporter account ID
    let reporter_account_id =
        get_user_account_id(&client, &reporter_email, &jira_token).await.ok().flatten();

    // Create issue fields
    let project_key = std::env::var("DEVDOCK_JIRA_PROJECT").unwrap_or("EN".into());
    let parent_key = parent_issue();
    let issue_type = if parent_key.is_empty() { "Bug" } else { "Sub-task" };

    let fields = JiraIssueFields {
        project: JiraProject { key: project_key },
        summary: format!("Devdock Bug Report - {summary}"),
        description,
        issuetype: JiraIssueType { name: issue_type.to_string() },
        parent: if parent_key.is_empty() { None } else { Some(JiraParent { key: parent_key }) },
        reporter: reporter_account_id.map(|id| JiraReporter { account_id: id }),
    };

    let issue_request = JiraCreateIssue { fields };

    // Create the issue
    let url = format!("{}/rest/api/3/issue", jira_url());
    let response = client
        .post(&url)
        .header("Authorization", format!("Basic {encoded}"))
        .header("Content-Type", "application/json")
        .json(&issue_request)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await?;
        return Err(DiagnosticsError::Jira(format!(
            "Failed to create issue (status {status}): {body}"
        )));
    }

    let issue_response: JiraIssueResponse = response.json().await?;
    let issue_key = issue_response.key.clone();

    info!("Created Jira issue: {}", issue_key);

    // Attach the archive
    attach_file_to_issue(&client, &issue_key, attachment_path, &encoded).await?;

    Ok(issue_key)
}

async fn attach_file_to_issue(
    client: &reqwest::Client,
    issue_key: &str,
    attachment_path: &Path,
    auth_header: &str,
) -> Result<()> {
    info!("Attaching archive to Jira issue {}", issue_key);

    let file_content = std::fs::read(attachment_path)?;
    let filename = attachment_path.file_name().and_then(|n| n.to_str()).unwrap_or("bug_report.zip");

    let part = multipart::Part::bytes(file_content).file_name(filename.to_string());
    let form = multipart::Form::new().part("file", part);

    let url = format!("{}/rest/api/3/issue/{issue_key}/attachments", jira_url());
    let response = client
        .post(&url)
        .header("Authorization", format!("Basic {auth_header}"))
        .header("X-Atlassian-Token", "no-check")
        .multipart(form)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await?;
        return Err(DiagnosticsError::Jira(format!(
            "Failed to attach file (status {status}): {body}"
        )));
    }

    info!("Successfully attached archive");
    Ok(())
}
