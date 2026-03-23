//! MCP tool definitions and implementations.

use anyhow::{Context, Result};
use serde_json::Value;
use std::process::Command;

/// Returns the list of all available MCP tools.
pub fn list_tools() -> Vec<Value> {
    vec![
        tool_def(
            "devdock_start",
            "Start a DevDock profile (dry-run or real)",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "profile": { "type": "string", "description": "Profile name to start" },
                    "services": { "type": "string", "description": "Comma-separated services to start" },
                    "dry_run": { "type": "boolean", "description": "If true, bootstrap only without starting Tilt", "default": true },
                    "mode": { "type": "string", "description": "Start mode: local or remote", "default": "local" }
                },
                "required": ["profile"]
            }),
        ),
        tool_def(
            "devdock_stop",
            "Stop the current DevDock instance",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "force": { "type": "boolean", "description": "Force stop without cleanup", "default": false },
                    "instance_id": { "type": "string", "description": "Specific instance ID to stop" }
                }
            }),
        ),
        tool_def(
            "devdock_status",
            "Get all resource health from the Tilt API",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        tool_def(
            "devdock_resource_status",
            "Get health status of a specific resource",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "resource_name": { "type": "string", "description": "Name of the resource" }
                },
                "required": ["resource_name"]
            }),
        ),
        tool_def(
            "devdock_logs",
            "Get recent logs for a service",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "service_name": { "type": "string", "description": "Name of the service" },
                    "lines": { "type": "integer", "description": "Number of log lines to return", "default": 50 }
                },
                "required": ["service_name"]
            }),
        ),
        tool_def(
            "devdock_profile_list",
            "List available profiles",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        tool_def(
            "devdock_profile_show",
            "Show details of a specific profile",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "profile_name": { "type": "string", "description": "Profile name to show" }
                },
                "required": ["profile_name"]
            }),
        ),
        tool_def(
            "devdock_validate",
            "Validate a DevDock profile",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "profile_name": { "type": "string", "description": "Profile name to validate" }
                },
                "required": ["profile_name"]
            }),
        ),
        tool_def(
            "devdock_plugin_create",
            "Scaffold a new DevDock plugin",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Plugin name" },
                    "hook": { "type": "string", "description": "Target lifecycle hook" },
                    "language": { "type": "string", "description": "Plugin language: rust, python, or bash", "default": "rust" }
                },
                "required": ["name", "hook"]
            }),
        ),
        tool_def(
            "devdock_plugin_list",
            "List installed plugins",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        tool_def(
            "devdock_lifecycle_state",
            "Get current lifecycle state",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        tool_def(
            "devdock_config_show",
            "Show devdock.toml contents",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
    ]
}

/// Dispatch a tool call to the appropriate handler.
pub async fn call_tool(name: &str, arguments: &Value) -> Result<Value> {
    match name {
        "devdock_start" => tool_start(arguments).await,
        "devdock_stop" => tool_stop(arguments).await,
        "devdock_status" => tool_status().await,
        "devdock_resource_status" => tool_resource_status(arguments).await,
        "devdock_logs" => tool_logs(arguments).await,
        "devdock_profile_list" => tool_profile_list().await,
        "devdock_profile_show" => tool_profile_show(arguments).await,
        "devdock_validate" => tool_validate(arguments).await,
        "devdock_plugin_create" => tool_plugin_create(arguments).await,
        "devdock_plugin_list" => tool_plugin_list().await,
        "devdock_lifecycle_state" => tool_lifecycle_state().await,
        "devdock_config_show" => tool_config_show().await,
        _ => anyhow::bail!("Unknown tool: {name}"),
    }
}

async fn tool_start(args: &Value) -> Result<Value> {
    let profile = args["profile"].as_str().context("profile is required")?;
    let dry_run = args["dry_run"].as_bool().unwrap_or(true);
    let services = args["services"].as_str();

    let mut cmd = Command::new("devdock");
    cmd.arg("start").arg(profile);
    if dry_run {
        cmd.arg("--dry-run");
    }
    if let Some(svcs) = services {
        cmd.arg("--services").arg(svcs);
    }

    let output = cmd.output().context("Failed to run devdock start")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    Ok(serde_json::json!({
        "success": output.status.success(),
        "stdout": stdout.to_string(),
        "stderr": stderr.to_string(),
    }))
}

async fn tool_stop(args: &Value) -> Result<Value> {
    let mut cmd = Command::new("devdock");
    cmd.arg("stop");
    if args["force"].as_bool().unwrap_or(false) {
        cmd.arg("--force");
    }
    if let Some(id) = args["instance_id"].as_str() {
        cmd.arg(id);
    }

    let output = cmd.output().context("Failed to run devdock stop")?;
    Ok(serde_json::json!({
        "success": output.status.success(),
        "output": String::from_utf8_lossy(&output.stdout).to_string(),
    }))
}

async fn tool_status() -> Result<Value> {
    // Query Tilt API for resource health
    let client =
        reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(5)).build()?;

    match client.get("http://localhost:10350/api/view").send() {
        Ok(resp) => {
            let data: Value = resp.json().context("Failed to parse Tilt API response")?;
            let mut resources = Vec::new();

            if let Some(ui_resources) = data.get("uiResources").and_then(|r| r.as_array()) {
                for r in ui_resources {
                    let name = r.pointer("/metadata/name").and_then(|n| n.as_str()).unwrap_or("?");
                    let runtime = r
                        .pointer("/status/runtimeStatus")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown");
                    let update = r
                        .pointer("/status/updateStatus")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown");

                    resources.push(serde_json::json!({
                        "name": name,
                        "runtime_status": runtime,
                        "update_status": update,
                    }));
                }
            }

            Ok(serde_json::json!({
                "tilt_running": true,
                "resources": resources,
            }))
        }
        Err(_) => Ok(serde_json::json!({
            "tilt_running": false,
            "message": "Tilt is not running or not accessible at http://localhost:10350",
        })),
    }
}

async fn tool_resource_status(args: &Value) -> Result<Value> {
    let name = args["resource_name"].as_str().context("resource_name is required")?;

    let status = tool_status().await?;
    if let Some(resources) = status.get("resources").and_then(|r| r.as_array()) {
        for r in resources {
            if r.get("name").and_then(|n| n.as_str()) == Some(name) {
                return Ok(r.clone());
            }
        }
    }

    Ok(serde_json::json!({
        "name": name,
        "error": format!("Resource '{}' not found", name),
    }))
}

async fn tool_logs(args: &Value) -> Result<Value> {
    let service = args["service_name"].as_str().context("service_name is required")?;
    let lines = args["lines"].as_u64().unwrap_or(50);

    let output = Command::new("docker")
        .args(["logs", &format!("devdock-{service}"), "--tail", &lines.to_string()])
        .output()
        .context("Failed to get docker logs")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    Ok(serde_json::json!({
        "service": service,
        "logs": if stdout.is_empty() { stderr.to_string() } else { stdout.to_string() },
    }))
}

async fn tool_profile_list() -> Result<Value> {
    let output = Command::new("devdock")
        .args(["profile", "list"])
        .output()
        .context("Failed to list profiles")?;

    Ok(serde_json::json!({
        "output": String::from_utf8_lossy(&output.stdout).to_string(),
    }))
}

async fn tool_profile_show(args: &Value) -> Result<Value> {
    let name = args["profile_name"].as_str().context("profile_name is required")?;

    let output = Command::new("devdock")
        .args(["profile", "show", name])
        .output()
        .context("Failed to show profile")?;

    Ok(serde_json::json!({
        "output": String::from_utf8_lossy(&output.stdout).to_string(),
    }))
}

async fn tool_validate(args: &Value) -> Result<Value> {
    let name = args["profile_name"].as_str().context("profile_name is required")?;

    let output = Command::new("devdock")
        .args(["validate", name])
        .output()
        .context("Failed to validate profile")?;

    Ok(serde_json::json!({
        "valid": output.status.success(),
        "output": String::from_utf8_lossy(&output.stdout).to_string(),
        "errors": String::from_utf8_lossy(&output.stderr).to_string(),
    }))
}

async fn tool_plugin_create(args: &Value) -> Result<Value> {
    let name = args["name"].as_str().context("name is required")?;
    let language = args["language"].as_str().unwrap_or("rust");

    let output = Command::new("devdock")
        .args(["plugin", "create", name, "--language", language])
        .output()
        .context("Failed to create plugin")?;

    Ok(serde_json::json!({
        "success": output.status.success(),
        "output": String::from_utf8_lossy(&output.stdout).to_string(),
    }))
}

async fn tool_plugin_list() -> Result<Value> {
    let output = Command::new("devdock")
        .args(["plugin", "list"])
        .output()
        .context("Failed to list plugins")?;

    Ok(serde_json::json!({
        "output": String::from_utf8_lossy(&output.stdout).to_string(),
    }))
}

async fn tool_lifecycle_state() -> Result<Value> {
    // Check if Tilt is running to infer lifecycle state
    let tilt_running = Command::new("curl")
        .args(["-s", "-o", "/dev/null", "-w", "%{http_code}", "http://localhost:10350/api/view"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "200")
        .unwrap_or(false);

    let state = if tilt_running { "running" } else { "stopped" };

    Ok(serde_json::json!({
        "state": state,
        "tilt_running": tilt_running,
    }))
}

async fn tool_config_show() -> Result<Value> {
    // Find devdock.toml
    let mut dir = std::env::current_dir()?;
    loop {
        let config_path = dir.join(".devdock/devdock.toml");
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            return Ok(serde_json::json!({
                "path": config_path.to_string_lossy(),
                "content": content,
            }));
        }
        if !dir.pop() {
            break;
        }
    }

    anyhow::bail!("No .devdock/devdock.toml found in any parent directory")
}

fn tool_def(name: &str, description: &str, input_schema: Value) -> Value {
    serde_json::json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}
