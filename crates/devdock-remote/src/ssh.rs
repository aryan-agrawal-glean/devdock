//! SSH utilities for remote workspace communication.

use crate::{RemoteError, Result};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Outcome of a locally spawned command that may be given a deadline.
#[derive(Debug)]
pub enum CommandOutcome {
    Completed { success: bool, exit_code: i32, stdout: String, stderr: String },
    TimedOut,
}

/// Run `cmd` with piped stdio and kill it if it exceeds `timeout`.
pub fn run_command_with_timeout(
    mut cmd: Command,
    timeout: Duration,
) -> std::io::Result<CommandOutcome> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let start = Instant::now();

    loop {
        match child.try_wait()? {
            Some(status) => {
                let mut stdout_buf = Vec::new();
                let mut stderr_buf = Vec::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_end(&mut stdout_buf);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_end(&mut stderr_buf);
                }
                return Ok(CommandOutcome::Completed {
                    success: status.success(),
                    exit_code: status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&stdout_buf).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr_buf).into_owned(),
                });
            }
            None => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(CommandOutcome::TimedOut);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn coder_ssh_command(workspace: &str, command: &str) -> Command {
    let mut cmd = Command::new("coder");
    cmd.arg("ssh").arg(workspace).arg("--").arg(command);
    cmd
}

fn log_ssh_result(
    log_file: Option<&Path>,
    workspace: &str,
    command: &str,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
) {
    let Some(log_path) = log_file else {
        return;
    };
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
    ssh_exec_with_log_timeout(workspace, command, log_file, None)
}

/// Execute a remote command and fail with [`RemoteError::SshTimeout`] if it exceeds `timeout_secs`.
pub fn ssh_exec_with_timeout(workspace: &str, command: &str, timeout_secs: u64) -> Result<String> {
    ssh_exec_with_log_timeout(workspace, command, None, Some(timeout_secs))
}

/// Execute a remote command with optional local logging and an optional deadline.
pub fn ssh_exec_with_log_timeout(
    workspace: &str,
    command: &str,
    log_file: Option<&Path>,
    timeout_secs: Option<u64>,
) -> Result<String> {
    info!("Executing on {}: {}", workspace, command);

    let (stdout, stderr, exit_code, success) = if let Some(timeout) = timeout_secs {
        let outcome = run_command_with_timeout(
            coder_ssh_command(workspace, command),
            Duration::from_secs(timeout),
        )
        .map_err(|e| RemoteError::SshError(format!("Failed to execute SSH command: {e}")))?;
        match outcome {
            CommandOutcome::TimedOut => {
                return Err(RemoteError::SshTimeout(timeout));
            }
            CommandOutcome::Completed { success, exit_code, stdout, stderr } => {
                (stdout, stderr, exit_code, success)
            }
        }
    } else {
        let output = coder_ssh_command(workspace, command)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| RemoteError::SshError(format!("Failed to execute SSH command: {e}")))?;
        (
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
            output.status.code().unwrap_or(-1),
            output.status.success(),
        )
    };

    log_ssh_result(log_file, workspace, command, exit_code, &stdout, &stderr);

    if !success {
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

    match ssh_exec_with_log_timeout(workspace, command, log_file, timeout_secs) {
        Ok(_) => Ok(true),
        Err(RemoteError::SshTimeout(secs)) => Err(RemoteError::SshTimeout(secs)),
        Err(RemoteError::SshError(msg)) if msg.starts_with("Command failed") => Ok(false),
        Err(e) => Err(e),
    }
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

    info!("Waiting for SSH connectivity to {}", workspace);

    let start = Instant::now();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_command_with_timeout_completes_quickly() {
        let outcome = run_command_with_timeout(Command::new("true"), Duration::from_secs(5))
            .expect("spawn true");
        match outcome {
            CommandOutcome::Completed { success, exit_code, .. } => {
                assert!(success);
                assert_eq!(exit_code, 0);
            }
            CommandOutcome::TimedOut => panic!("true should not time out"),
        }
    }

    #[test]
    fn run_command_with_timeout_kills_sleep() {
        let start = Instant::now();
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let outcome =
            run_command_with_timeout(cmd, Duration::from_millis(400)).expect("spawn sleep");
        assert!(matches!(outcome, CommandOutcome::TimedOut));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timeout must return in well under the child sleep"
        );
    }

    #[test]
    fn run_command_captures_stdout() {
        let outcome = run_command_with_timeout(
            {
                let mut cmd = Command::new("printf");
                cmd.arg("hello-diag");
                cmd
            },
            Duration::from_secs(5),
        )
        .expect("spawn printf");
        match outcome {
            CommandOutcome::Completed { stdout, success, .. } => {
                assert!(success);
                assert!(stdout.contains("hello-diag"));
            }
            CommandOutcome::TimedOut => panic!("printf should not time out"),
        }
    }
}
