//! SSH utilities for remote workspace communication.

use crate::{process::output_with_timeout, RemoteError, Result};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use tracing::{debug, info};

/// Execute a command on a remote workspace via SSH.
pub fn ssh_exec(workspace: &str, command: &str) -> Result<String> {
    ssh_exec_with_log(workspace, command, None)
}

/// Execute a command on a remote workspace via SSH with optional logging.
pub fn ssh_exec_with_log(
    workspace: &str,
    command: &str,
    log_file: Option<&Path>,
) -> Result<String> {
    info!("Executing on {}: {}", workspace, command);

    // Pass command as-is. Coder SSH handles the shell invocation automatically.
    let mut coder = Command::new("coder");
    coder.arg("ssh").arg(workspace).arg("--").arg(command);
    let output = output_with_timeout(&mut coder, "coder ssh")
        .map_err(|e| RemoteError::SshError(format!("Failed to execute SSH command: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    // Log to file if provided
    if let Some(log_path) = log_file {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
            let _ = writeln!(file, "\n=== SSH Command on {workspace} ===");
            let _ = writeln!(file, "Command: {command}");
            let _ = writeln!(file, "Exit code: {exit_code}");
            if !stdout.is_empty() {
                let _ = writeln!(file, "STDOUT:\n{stdout}");
            }
            if !stderr.is_empty() {
                let _ = writeln!(file, "STDERR:\n{stderr}");
            }
        }
    }

    if !output.status.success() {
        return Err(RemoteError::SshError(format!("Command failed (exit {exit_code}): {stderr}")));
    }

    debug!("SSH output: {}", stdout);
    Ok(stdout)
}

/// Execute a command and check if it succeeds (for grep -q style checks).
pub fn ssh_check(workspace: &str, command: &str, log_file: Option<&Path>) -> Result<bool> {
    ssh_check_with_timeout(workspace, command, log_file, None)
}

/// Execute a command and check if it succeeds with optional timeout.
pub fn ssh_check_with_timeout(
    workspace: &str,
    command: &str,
    log_file: Option<&Path>,
    timeout_secs: Option<u64>,
) -> Result<bool> {
    debug!("Checking on {}: {}", workspace, command);

    let mut cmd = Command::new("coder");
    cmd.arg("ssh").arg(workspace).arg("--").arg(command);

    let output = if let Some(timeout) = timeout_secs {
        // Spawn and wait with timeout
        use std::time::Duration;
        let mut child = cmd
            .spawn()
            .map_err(|e| RemoteError::SshError(format!("Failed to spawn SSH command: {e}")))?;

        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    // Process completed
                    let exit_code = status.code().unwrap_or(-1);
                    let success = status.success();

                    // Log to file if provided
                    if let Some(log_path) = log_file {
                        if let Ok(mut file) =
                            OpenOptions::new().create(true).append(true).open(log_path)
                        {
                            let _ = writeln!(file, "\n=== SSH Check on {workspace} ===");
                            let _ = writeln!(file, "Command: {command}");
                            let _ = writeln!(file, "Exit code: {exit_code}");
                            let _ = writeln!(file, "Success: {success}");
                        }
                    }

                    return Ok(success);
                }
                Ok(None) => {
                    // Still running
                    if start.elapsed() > Duration::from_secs(timeout) {
                        // Timeout - kill the process
                        let _ = child.kill();
                        return Err(RemoteError::SshError(format!(
                            "SSH command timed out after {timeout}s"
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    return Err(RemoteError::SshError(format!(
                        "Failed to wait for SSH command: {e}"
                    )));
                }
            }
        }
    } else {
        // No timeout, use normal output()
        let out = output_with_timeout(&mut cmd, "coder ssh check")
            .map_err(|e| RemoteError::SshError(format!("Failed to execute SSH command: {e}")))?;
        out
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);
    let success = output.status.success();

    // Log to file if provided
    if let Some(log_path) = log_file {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
            let _ = writeln!(file, "\n=== SSH Check on {workspace} ===");
            let _ = writeln!(file, "Command: {command}");
            let _ = writeln!(file, "Exit code: {exit_code}");
            let _ = writeln!(file, "Success: {success}");
            if !stdout.is_empty() {
                let _ = writeln!(file, "STDOUT:\n{stdout}");
            }
            if !stderr.is_empty() {
                let _ = writeln!(file, "STDERR:\n{stderr}");
            }
        }
    }

    Ok(success)
}

/// Check if SSH connection to workspace is available.
pub fn check_ssh_connectivity(workspace: &str) -> Result<bool> {
    match ssh_exec(workspace, "echo 'test'") {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Wait for SSH to become available (with timeout).
pub fn wait_for_ssh_ready(workspace: &str, timeout_secs: u64) -> Result<()> {
    use std::thread::sleep;
    use std::time::Duration;

    info!("Waiting for SSH connectivity to {}", workspace);

    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(timeout_secs);

    loop {
        if check_ssh_connectivity(workspace)? {
            info!("SSH ready for {}", workspace);
            return Ok(());
        }

        if start.elapsed() > timeout {
            return Err(RemoteError::SshError(format!(
                "SSH connection timeout after {timeout_secs} seconds"
            )));
        }

        sleep(Duration::from_secs(5));
    }
}
