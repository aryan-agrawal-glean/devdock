//! Coder CLI wrapper for workspace management.

use crate::{process::output_with_timeout, RemoteError, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use tracing::{debug, info, warn};

pub const PORT_FORWARD_BIND_ADDRESS: &str = devdock_state::LOOPBACK_BIND_ADDRESS;

pub fn port_forward_tcp_spec(local_port: u16, remote_port: u16) -> String {
    format!("{PORT_FORWARD_BIND_ADDRESS}:{local_port}:{remote_port}")
}

/// Render the exact command users can run to recreate a forward manually.
///
/// Keep this beside the process-spawning implementation so recovery output and
/// managed forwards cannot drift on the bind address or Coder argument order.
pub fn port_forward_command_line(workspace: &str, local_port: u16, remote_port: u16) -> String {
    format!(
        "coder port-forward {workspace} --tcp {}",
        port_forward_tcp_spec(local_port, remote_port)
    )
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

    let output = output_with_timeout(&mut cmd, "coder create")
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

    let mut command = Command::new("coder");
    command.arg("start").arg(name).arg("--yes");
    let output = output_with_timeout(&mut command, "coder start")
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

    let mut command = Command::new("coder");
    command.arg("stop").arg(name).arg("--yes");
    let output = output_with_timeout(&mut command, "coder stop")
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

    let mut command = Command::new("coder");
    command.arg("delete").arg(name).arg("--yes");
    let output = output_with_timeout(&mut command, "coder delete")
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

    let mut command = Command::new("coder");
    command
        .arg("list")
        .arg("--output")
        .arg("json")
        .arg("--search")
        .arg(format!("owner:me name:{search_name}"));
    let output = output_with_timeout(&mut command, "coder list")
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

/// Get workspace status.
pub fn get_workspace_status(name: &str) -> Result<String> {
    check_coder_installed()?;

    let mut command = Command::new("coder");
    command.arg("list").arg("--output").arg("json");
    let output = output_with_timeout(&mut command, "coder list")
        .map_err(|e| RemoteError::CommandFailed(format!("Failed to list workspaces: {e}")))?;

    if !output.status.success() {
        return Err(RemoteError::CommandFailed("Failed to get workspace status".to_string()));
    }

    // Parse JSON output and find workspace
    let stdout = String::from_utf8_lossy(&output.stdout);
    // For now, return simple status - can be enhanced with proper JSON parsing
    Ok(if stdout.contains(name) { "running" } else { "stopped" }.to_string())
}

/// Start a single port forward for a workspace.
pub fn port_forward_single(workspace: &str, local_port: u16, remote_port: u16) -> Result<Child> {
    check_coder_installed()?;

    info!("Port forwarding {}:{} -> {}", workspace, remote_port, local_port);

    let mut command = port_forward_command(workspace, local_port, remote_port);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let child = spawn_detached_port_forward(&mut command)?;

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
    command.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
    spawn_detached_port_forward(&mut command)
}

/// Spawn an owned Coder forwarding child in its own session. Non-interactive
/// command runners can clean up their own process group when the parent exits;
/// a managed forward must not be part of that group because its persisted exact
/// PID/start-time/command identity owns its later cleanup.
fn spawn_detached_port_forward(command: &mut Command) -> Result<Child> {
    detach_port_forward_process(command);
    command.spawn().map_err(|error| {
        RemoteError::PortForwardError(format!("Failed to start port forward: {error}"))
    })
}

#[cfg(unix)]
fn detach_port_forward_process(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: `pre_exec` runs in the child after fork and before exec. `setsid`
    // is an async-signal-safe libc call and is the only operation in the hook.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(not(unix))]
fn detach_port_forward_process(_command: &mut Command) {
    // Windows child-process group cleanup does not use Unix sessions. The
    // exact persisted PID and command fingerprint remain the ownership check.
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
    fn renders_exact_manual_port_forward_command() {
        assert_eq!(
            port_forward_command_line("dev-1afa81a3", 11350, 10350),
            "coder port-forward dev-1afa81a3 --tcp 127.0.0.1:11350:10350"
        );
    }

    #[cfg(unix)]
    #[test]
    fn managed_forward_child_survives_a_launch_wrapper_exit_in_its_own_session() {
        let caller_session = unsafe { libc::getsid(0) };
        assert_ne!(caller_session, -1);

        let temp_dir = tempfile::tempdir().unwrap();
        let child_pid_path = temp_dir.path().join("forward-child.pid");
        let mut command = Command::new("sh");
        // The shell models a noninteractive launcher. It exits as soon as it
        // starts the long-lived forwarding child, which must remain in the
        // independent session created by `spawn_detached_port_forward`.
        command
            .arg("-c")
            .arg("sleep 5 & printf '%s' \"$!\" > \"$1\"")
            .arg("forward-launcher")
            .arg(&child_pid_path);
        let mut child = spawn_detached_port_forward(&mut command).unwrap();
        let launcher_pid = child.id() as libc::pid_t;
        assert!(child.wait().unwrap().success(), "launch wrapper should exit cleanly");
        let forward_pid = std::fs::read_to_string(&child_pid_path)
            .unwrap()
            .trim()
            .parse::<libc::pid_t>()
            .unwrap();
        let forward_session = unsafe { libc::getsid(forward_pid) };

        assert_eq!(forward_session, launcher_pid);
        assert_ne!(forward_session, caller_session);
        assert_eq!(
            unsafe { libc::kill(forward_pid, 0) },
            0,
            "forward remains alive after wrapper exit"
        );

        let _ = unsafe { libc::kill(forward_pid, libc::SIGTERM) };
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
}
