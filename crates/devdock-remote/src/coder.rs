//! Coder CLI wrapper for workspace management.

use crate::ssh::{run_command_with_timeout, CommandOutcome};
use crate::{RemoteError, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tracing::{debug, info, warn};

pub const PORT_FORWARD_BIND_ADDRESS: &str = devdock_state::LOOPBACK_BIND_ADDRESS;

pub fn port_forward_tcp_spec(local_port: u16, remote_port: u16) -> String {
    format!("{PORT_FORWARD_BIND_ADDRESS}:{local_port}:{remote_port}")
}

/// Configuration for creating a Coder workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoderWorkspaceConfig {
    /// Coder template name (always "devdock-glean")
    pub template: String,
    /// Workspace name (user-provided or branch name)
    pub workspace_name: String,
    /// GCP region (default: us-east4-a)
    pub region: String,
    /// Local port offset (not passed to Coder, used for port forwarding calculation)
    pub port_base: Option<u16>,
    /// DevDock profile name
    pub profile: String,
    /// Git branch to checkout in remote workspace
    pub branch: String,
    /// Data disk size in GB (default: 300)
    pub disk_size: Option<u32>,
}

/// Immutable identity returned by the Coder API via `coder list --output json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CoderWorkspaceIdentity {
    pub id: String,
    pub name: String,
    pub owner_name: String,
}

/// Check if coder CLI is installed and available.
pub fn check_coder_installed() -> Result<()> {
    which::which("coder").map_err(|_| RemoteError::CoderNotInstalled)?;

    debug!("Coder CLI found");
    Ok(())
}

fn region_to_preset(region: &str) -> &str {
    match region {
        "us-east4-a" => "us-east",
        "us-west1-a" => "us-west",
        "us-central1-a" => "us-central",
        _ => "us-east",
    }
}

/// Create a new Coder workspace.
pub fn create_workspace(config: &CoderWorkspaceConfig) -> Result<String> {
    check_coder_installed()?;

    info!("Creating Coder workspace: {}", config.workspace_name);

    let preset = region_to_preset(&config.region);
    let mut cmd = Command::new("coder");
    cmd.arg("create")
        .arg(&config.workspace_name)
        .arg("--template")
        .arg(&config.template)
        .arg("--parameter")
        .arg(format!("gcp_region={}", config.region))
        .arg("--parameter")
        .arg("jetbrains_ide=IU") // IntelliJ IDEA Ultimate
        .arg("--parameter")
        .arg("dotfiles_uri=") // Empty dotfiles
        .arg("--parameter")
        .arg("github_username=") // Empty default
        .arg("--parameter")
        .arg(format!("branch_name={}", config.branch)) // Use specified branch
        .arg("--parameter")
        .arg("force_fresh_data=false")
        .arg("--parameter")
        .arg(format!("data_disk_size={}", config.disk_size.unwrap_or(300)))
        .arg("--preset")
        .arg(preset)
        .arg("--yes"); // Non-interactive

    debug!("Running: {:?}", cmd);

    let output = cmd
        .output()
        .map_err(|e| RemoteError::CommandFailed(format!("Failed to run coder create: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RemoteError::WorkspaceCreationFailed(stderr.to_string()));
    }

    info!("Workspace created: {}", config.workspace_name);
    Ok(config.workspace_name.clone())
}

/// Start an existing Coder workspace.
pub fn start_workspace(name: &str) -> Result<String> {
    check_coder_installed()?;

    info!("Starting Coder workspace: {}", name);

    let output = Command::new("coder")
        .arg("start")
        .arg(name)
        .arg("--yes")
        .output()
        .map_err(|e| RemoteError::CommandFailed(format!("Failed to start workspace: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RemoteError::WorkspaceNotFound(stderr.to_string()));
    }

    info!("Workspace started: {}", name);
    Ok(name.to_string())
}

/// Stop a Coder workspace.
pub fn stop_workspace(name: &str) -> Result<()> {
    check_coder_installed()?;

    info!("Stopping Coder workspace: {}", name);

    let output = Command::new("coder")
        .arg("stop")
        .arg(name)
        .arg("--yes")
        .output()
        .map_err(|e| RemoteError::CommandFailed(format!("Failed to stop workspace: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!("Failed to stop workspace: {}", stderr);
    }

    Ok(())
}

/// Delete a Coder workspace.
pub fn delete_workspace(name: &str) -> Result<()> {
    check_coder_installed()?;

    info!("Deleting Coder workspace: {}", name);

    let output = Command::new("coder")
        .arg("delete")
        .arg(name)
        .arg("--yes")
        .output()
        .map_err(|e| RemoteError::CommandFailed(format!("Failed to delete workspace: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!("Failed to delete workspace: {}", stderr);
    }

    Ok(())
}

/// Check if a workspace exists.
pub fn workspace_exists(name: &str) -> Result<bool> {
    Ok(get_workspace_identity(name)?.is_some())
}

/// Resolve a workspace name to its immutable Coder workspace ID.
pub fn get_workspace_identity(name: &str) -> Result<Option<CoderWorkspaceIdentity>> {
    check_coder_installed()?;
    let search_name = name.rsplit('/').next().unwrap_or(name);

    let output = Command::new("coder")
        .arg("list")
        .arg("--output")
        .arg("json")
        .arg("--search")
        .arg(format!("owner:me name:{search_name}"))
        .output()
        .map_err(|e| RemoteError::CommandFailed(format!("Failed to list workspaces: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RemoteError::CommandFailed(format!(
            "Failed to resolve Coder workspace identity: {stderr}"
        )));
    }

    parse_workspace_identity(&output.stdout, name)
}

fn parse_workspace_identity(
    bytes: &[u8],
    requested: &str,
) -> Result<Option<CoderWorkspaceIdentity>> {
    let workspaces: Vec<CoderWorkspaceIdentity> =
        serde_json::from_slice(bytes).map_err(|error| {
            RemoteError::CommandFailed(format!("Invalid JSON from coder list: {error}"))
        })?;
    Ok(workspaces.into_iter().find(|workspace| {
        workspace.name == requested
            || format!("{}/{}", workspace.owner_name, workspace.name) == requested
    }))
}

#[derive(Debug, Deserialize)]
struct CoderListStatusEntry {
    name: String,
    #[serde(default)]
    owner_name: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    latest_build: Option<CoderLatestBuild>,
}

#[derive(Debug, Deserialize)]
struct CoderLatestBuild {
    #[serde(default)]
    status: Option<String>,
}

fn names_match(entry: &CoderListStatusEntry, requested: &str) -> bool {
    if entry.name == requested {
        return true;
    }
    if let Some(owner) = &entry.owner_name {
        return format!("{owner}/{}", entry.name) == requested;
    }
    false
}

/// Parse `coder list --output json` for a workspace's latest build status.
pub fn parse_workspace_runtime_status(bytes: &[u8], requested: &str) -> Result<Option<String>> {
    let entries: Vec<CoderListStatusEntry> = serde_json::from_slice(bytes).map_err(|error| {
        RemoteError::CommandFailed(format!("Invalid JSON from coder list: {error}"))
    })?;
    Ok(entries
        .into_iter()
        .find(|entry| names_match(entry, requested))
        .and_then(|entry| entry.latest_build.and_then(|build| build.status).or(entry.status)))
}

/// Get workspace status (best-effort; prefers latest_build.status).
pub fn get_workspace_status(name: &str) -> Result<String> {
    match get_workspace_runtime_status(name, 15)? {
        Some(status) => Ok(status),
        None => Ok("stopped".to_string()),
    }
}

/// Timed `coder list` lookup of a workspace's runtime status (`running`, `stopped`, ...).
pub fn get_workspace_runtime_status(name: &str, timeout_secs: u64) -> Result<Option<String>> {
    check_coder_installed()?;
    let search_name = name.rsplit('/').next().unwrap_or(name);

    let mut cmd = Command::new("coder");
    cmd.arg("list")
        .arg("--output")
        .arg("json")
        .arg("--search")
        .arg(format!("owner:me name:{search_name}"));

    let outcome = run_command_with_timeout(cmd, Duration::from_secs(timeout_secs))
        .map_err(|e| RemoteError::CommandFailed(format!("Failed to list workspaces: {e}")))?;

    match outcome {
        CommandOutcome::TimedOut => {
            Err(RemoteError::CommandFailed(format!("coder list timed out after {timeout_secs}s")))
        }
        CommandOutcome::Completed { success, stdout, stderr, .. } => {
            if !success {
                return Err(RemoteError::CommandFailed(format!(
                    "Failed to get workspace status: {stderr}"
                )));
            }
            parse_workspace_runtime_status(stdout.as_bytes(), name)
        }
    }
}

/// Start a single port forward for a workspace.
pub fn port_forward_single(workspace: &str, local_port: u16, remote_port: u16) -> Result<Child> {
    check_coder_installed()?;

    info!("Port forwarding {}:{} -> {}", workspace, remote_port, local_port);

    let mut command = port_forward_command(workspace, local_port, remote_port);
    let child =
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(
            |e| RemoteError::PortForwardError(format!("Failed to start port forward: {e}")),
        )?;

    debug!("Port forward started with PID: {}", child.id());
    Ok(child)
}

/// Start a managed forward while preserving the existing manual-command log.
pub fn port_forward_single_logged(
    workspace: &str,
    local_port: u16,
    remote_port: u16,
    log_path: &Path,
) -> Result<Child> {
    check_coder_installed()?;
    let log = File::create(log_path).map_err(|error| {
        RemoteError::PortForwardError(format!(
            "Failed to create port-forward log {}: {error}",
            log_path.display()
        ))
    })?;
    let mut command = port_forward_command(workspace, local_port, remote_port);
    command.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log).spawn().map_err(|error| {
        RemoteError::PortForwardError(format!("Failed to start port forward: {error}"))
    })
}

fn port_forward_command(workspace: &str, local_port: u16, remote_port: u16) -> Command {
    let mut command = Command::new("coder");
    command
        .arg("port-forward")
        .arg(workspace)
        .arg("--tcp")
        .arg(port_forward_tcp_spec(local_port, remote_port));
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_forward_spec_is_explicitly_ipv4_loopback_bound() {
        assert_eq!(port_forward_tcp_spec(11350, 10350), "127.0.0.1:11350:10350");
    }

    #[test]
    fn parses_exact_coder_workspace_identity() {
        let json = br#"[
          {"id":"workspace-id","name":"feature-one","owner_name":"owner"},
          {"id":"other-id","name":"feature-two","owner_name":"owner"}
        ]"#;

        let identity = parse_workspace_identity(json, "owner/feature-one").unwrap().unwrap();
        assert_eq!(identity.id, "workspace-id");
        assert_eq!(identity.name, "feature-one");
    }

    #[test]
    fn does_not_accept_partial_workspace_names() {
        let json = br#"[
          {"id":"workspace-id","name":"feature-one-copy","owner_name":"owner"}
        ]"#;
        assert!(parse_workspace_identity(json, "feature-one").unwrap().is_none());
    }

    #[test]
    fn parse_workspace_runtime_status_reads_latest_build() {
        let json = br#"[
          {
            "id":"workspace-id",
            "name":"feature-one",
            "owner_name":"owner",
            "latest_build": { "status": "stopped" }
          }
        ]"#;
        let status = parse_workspace_runtime_status(json, "feature-one").unwrap();
        assert_eq!(status.as_deref(), Some("stopped"));
    }

    #[test]
    fn parse_workspace_runtime_status_falls_back_to_top_level_status() {
        let json = br#"[{"name":"ws","status":"running"}]"#;
        let status = parse_workspace_runtime_status(json, "ws").unwrap();
        assert_eq!(status.as_deref(), Some("running"));
    }

    #[test]
    fn parse_workspace_runtime_status_missing_workspace() {
        let json = br#"[{"name":"other","latest_build":{"status":"running"}}]"#;
        let status = parse_workspace_runtime_status(json, "ws").unwrap();
        assert!(status.is_none());
    }
}
