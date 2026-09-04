//! Remote start/status diagnostics: durable child logs and bounded SSH probes.

use crate::coder;
use crate::ssh;
use crate::{RemoteError, Result};

/// Latest-start symlink on the remote workspace (points at the per-session log).
pub const REMOTE_START_LOG: &str = "/tmp/devdock-start.log";
/// Latest-start exit-status symlink.
pub const REMOTE_START_EXIT: &str = "/tmp/devdock-start.exit";
/// Latest-start metadata symlink (stage, exit code).
pub const REMOTE_START_META: &str = "/tmp/devdock-start.meta";

/// Lines of child output shown on tmux_dead.
pub const REMOTE_START_LOG_TAIL_LINES: usize = 40;

/// SSH budget for `devdock status --detailed` remote probes.
pub const REMOTE_STATUS_SSH_TIMEOUT_SECS: u64 = 10;
/// Budget for `coder list` before falling through to SSH.
pub const REMOTE_STATUS_CODER_LIST_TIMEOUT_SECS: u64 = 8;
/// Per-iteration SSH budget while waiting for Tilt after remote start.
pub const REMOTE_TILT_WAIT_SSH_TIMEOUT_SECS: u64 = 10;

/// Result of a remote Tilt/agent probe used by `status --detailed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteTiltProbe {
    TiltRunning,
    TiltStopped,
    WorkspaceStopped,
    AgentUnavailable { detail: String },
    TimedOut { timeout_secs: u64 },
}

impl RemoteTiltProbe {
    /// Stable user-facing state label (never a hang).
    pub fn label(&self) -> &'static str {
        match self {
            Self::TiltRunning => "Tilt is running",
            Self::TiltStopped => "Tilt is not running",
            Self::WorkspaceStopped => "workspace stopped",
            Self::AgentUnavailable { .. } => "agent unavailable",
            Self::TimedOut { .. } => "timed out",
        }
    }
}

/// Parsed diagnostics from a `DEVDOCK_STATUS:tmux_dead` SSH check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxDeadDiagnostics {
    pub exit_status: Option<i32>,
    pub log_exists: bool,
    pub log_path: Option<String>,
    pub log_tail: String,
}

fn sanitize_session_id(devdock_id: &str) -> String {
    let mut safe: String = devdock_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    if safe.is_empty() {
        safe.push_str("session");
    }
    safe
}

/// Per-session remote start log path (created before tmux detach).
pub fn remote_start_log_path(devdock_id: &str) -> String {
    format!("/tmp/devdock-start-{}.log", sanitize_session_id(devdock_id))
}

pub fn remote_start_exit_path(devdock_id: &str) -> String {
    format!("/tmp/devdock-start-{}.exit", sanitize_session_id(devdock_id))
}

pub fn remote_start_meta_path(devdock_id: &str) -> String {
    format!("/tmp/devdock-start-{}.meta", sanitize_session_id(devdock_id))
}

/// Build the remote script that creates the log, then detaches `devdock start` in tmux.
///
/// The log/exit/meta files are created **before** `tmux new-session -d` so a
/// dead session still has a path that exists. Inner stdout/stderr are redirected
/// into that log, and the inner exit code is written to the exit file.
pub fn build_remote_start_script(
    env_exports: &str,
    profile: &str,
    services: Option<&str>,
    log_path: &str,
    exit_path: &str,
    meta_path: &str,
) -> String {
    let start_cmd = match services {
        Some(svc) => format!("devdock start {profile} -s {svc}"),
        None => format!("devdock start {profile}"),
    };

    format!(
        r#"{env_exports}
cd /home/workspace/scio
umask 077
: > '{log_path}'
: > '{exit_path}'
: > '{meta_path}'
rm -f '{latest_log}' '{latest_exit}' '{latest_meta}'
ln -s '{log_path}' '{latest_log}' 2>/dev/null || true
ln -s '{exit_path}' '{latest_exit}' 2>/dev/null || true
ln -s '{meta_path}' '{latest_meta}' 2>/dev/null || true
if [ ! -f '{log_path}' ]; then
    echo "DEVDOCK_LOG_CREATED:0"
    echo "Failed to create remote start log {log_path}" >&2
    exit 1
fi
{{
    echo "=== DevDock remote start ==="
    echo "profile={profile}"
    echo "log={log_path}"
    echo "created=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}} >> '{log_path}'
printf '%s\n' 'stage=remote_devdock_start' 'log={log_path}' > '{meta_path}'
echo "DEVDOCK_LOG_CREATED:1"
echo "DEVDOCK_LOG_PATH:{log_path}"
tmux new-session -d -s devdock "{start_cmd} >> '{log_path}' 2>&1; ec=\$?; echo \$ec > '{exit_path}'; echo exit_code=\$ec >> '{meta_path}'; echo status=exited >> '{meta_path}'"
"#,
        latest_log = REMOTE_START_LOG,
        latest_exit = REMOTE_START_EXIT,
        latest_meta = REMOTE_START_META,
    )
}

/// Combined tmux + Tilt check. On tmux_dead, emit exit status and a bounded log tail.
pub fn tilt_wait_check_script(log_path: &str, exit_path: &str) -> String {
    format!(
        r#"
if ! tmux has-session -t devdock 2>/dev/null; then
    echo "DEVDOCK_STATUS:tmux_dead"
    if [ -s '{exit_path}' ]; then
        echo "DEVDOCK_EXIT:$(tr -d '[:space:]' < '{exit_path}')"
    else
        echo "DEVDOCK_EXIT:unknown"
    fi
    if [ -f '{log_path}' ]; then
        echo "DEVDOCK_LOG_EXISTS:1"
        echo "DEVDOCK_LOG_PATH:{log_path}"
        echo "DEVDOCK_LOG_TAIL_BEGIN"
        tail -n {tail} '{log_path}' 2>/dev/null || true
        echo "DEVDOCK_LOG_TAIL_END"
    else
        echo "DEVDOCK_LOG_EXISTS:0"
    fi
    exit 0
fi
if ss -tlnp 2>/dev/null | grep -q ':10350 ' 2>/dev/null; then
    echo "DEVDOCK_STATUS:tilt_ready"
    exit 0
fi
echo "DEVDOCK_STATUS:waiting"
exit 0
"#,
        tail = REMOTE_START_LOG_TAIL_LINES,
    )
}

/// Parse tmux_dead markers from an SSH check. `log_path` is only set when the file exists.
pub fn parse_tmux_dead_diagnostics(output: &str) -> TmuxDeadDiagnostics {
    let mut exit_status = None;
    let mut log_exists = false;
    let mut log_path = None;
    let mut capturing_tail = false;
    let mut tail_lines = Vec::new();

    for line in output.lines() {
        if line == "DEVDOCK_LOG_TAIL_BEGIN" {
            capturing_tail = true;
            continue;
        }
        if line == "DEVDOCK_LOG_TAIL_END" {
            capturing_tail = false;
            continue;
        }
        if capturing_tail {
            tail_lines.push(line.to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("DEVDOCK_EXIT:") {
            let trimmed = rest.trim();
            if trimmed != "unknown" && !trimmed.is_empty() {
                exit_status = trimmed.parse().ok();
            }
        } else if line == "DEVDOCK_LOG_EXISTS:1" {
            log_exists = true;
        } else if line == "DEVDOCK_LOG_EXISTS:0" {
            log_exists = false;
        } else if let Some(path) = line.strip_prefix("DEVDOCK_LOG_PATH:") {
            let path = path.trim();
            if !path.is_empty() {
                log_path = Some(path.to_string());
            }
        }
    }

    if !log_exists {
        log_path = None;
    }

    TmuxDeadDiagnostics { exit_status, log_exists, log_path, log_tail: tail_lines.join("\n") }
}

/// Human-readable tmux_dead report. Advertises a log path only when it exists.
pub fn format_tmux_dead_message(
    workspace: &str,
    profile: &str,
    diag: &TmuxDeadDiagnostics,
) -> String {
    let mut lines = Vec::new();
    lines.push("DevDock tmux session died while waiting for Tilt!".to_string());
    match diag.exit_status {
        Some(code) => lines.push(format!("Inner `devdock start` exit status: {code}")),
        None => lines.push(
            "Inner `devdock start` exit status: unknown (not written before the session died)"
                .to_string(),
        ),
    }

    if diag.log_exists {
        if let Some(path) = &diag.log_path {
            lines.push(format!("Start log: {path}"));
            lines.push(format!("  coder ssh {workspace} -- tail -n 80 {path}"));
        }
        if !diag.log_tail.trim().is_empty() {
            lines.push("Last log lines:".to_string());
            lines.push(diag.log_tail.clone());
        }
    } else {
        lines.push("No remote start log was captured (file was not created).".to_string());
        lines.push(format!("  coder ssh {workspace} -- tmux ls"));
    }

    lines.push(format!(
        "Manual retry: coder ssh {workspace} -- 'cd /home/workspace/scio && devdock start {profile}'"
    ));
    lines.join("\n")
}

pub fn command_failed_for_tmux_dead(diag: &TmuxDeadDiagnostics) -> String {
    let exit = diag.exit_status.map(|c| c.to_string()).unwrap_or_else(|| "unknown".to_string());
    match &diag.log_path {
        Some(path) if diag.log_exists => {
            format!("DevDock process died during startup (exit {exit}; log {path})")
        }
        _ => format!("DevDock process died during startup (exit {exit}; no start log captured)"),
    }
}

pub fn start_log_created(ssh_output: &str) -> bool {
    ssh_output.contains("DEVDOCK_LOG_CREATED:1")
}

pub fn is_stopped_workspace_status(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "stopped" | "canceled" | "cancelled" | "deleted" | "offline"
    )
}

/// Classify SSH probe failures into workspace stopped / agent unavailable / timed out.
pub fn classify_ssh_probe_failure(err: &RemoteError, timeout_secs: u64) -> RemoteTiltProbe {
    match err {
        RemoteError::SshTimeout(secs) => RemoteTiltProbe::TimedOut { timeout_secs: *secs },
        other => {
            let msg = other.to_string();
            let lower = msg.to_ascii_lowercase();
            if lower.contains("timed out") || lower.contains("timeout") {
                return RemoteTiltProbe::TimedOut { timeout_secs };
            }
            if lower.contains("workspace")
                && (lower.contains("stopped")
                    || lower.contains("shutdown")
                    || lower.contains("not found")
                    || lower.contains("deleted"))
            {
                return RemoteTiltProbe::WorkspaceStopped;
            }
            if lower.contains("stopped") && !lower.contains("tilt") {
                return RemoteTiltProbe::WorkspaceStopped;
            }
            RemoteTiltProbe::AgentUnavailable { detail: msg }
        }
    }
}

/// Probe remote Tilt without hanging: coder list (timed) then SSH (timed).
pub fn probe_remote_tilt(workspace: &str, ssh_timeout_secs: u64) -> RemoteTiltProbe {
    probe_remote_tilt_with(
        workspace,
        ssh_timeout_secs,
        coder::get_workspace_runtime_status,
        ssh::ssh_exec_with_timeout,
    )
}

pub fn probe_remote_tilt_with<ListFn, SshFn>(
    workspace: &str,
    ssh_timeout_secs: u64,
    mut list_status: ListFn,
    mut ssh_exec: SshFn,
) -> RemoteTiltProbe
where
    ListFn: FnMut(&str, u64) -> Result<Option<String>>,
    SshFn: FnMut(&str, &str, u64) -> Result<String>,
{
    match list_status(workspace, REMOTE_STATUS_CODER_LIST_TIMEOUT_SECS) {
        Ok(Some(status)) if is_stopped_workspace_status(&status) => {
            return RemoteTiltProbe::WorkspaceStopped;
        }
        Ok(Some(status))
            if status.eq_ignore_ascii_case("failed") || status.eq_ignore_ascii_case("error") =>
        {
            return RemoteTiltProbe::AgentUnavailable {
                detail: format!("Coder workspace status: {status}"),
            };
        }
        _ => {}
    }

    let check = "lsof -i:10350 >/dev/null 2>&1 && echo running || echo stopped";
    match ssh_exec(workspace, check, ssh_timeout_secs) {
        Ok(output) if output.trim() == "running" => RemoteTiltProbe::TiltRunning,
        Ok(_) => RemoteTiltProbe::TiltStopped,
        Err(err) => classify_ssh_probe_failure(&err, ssh_timeout_secs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_script_creates_log_before_tmux_and_redirects_child() {
        let script = build_remote_start_script(
            "export DEVDOCK_ID='abc'",
            "default_profile",
            None,
            "/tmp/devdock-start-abc.log",
            "/tmp/devdock-start-abc.exit",
            "/tmp/devdock-start-abc.meta",
        );

        let create_at = script.find(": > '/tmp/devdock-start-abc.log'").expect("create log");
        let tmux_at = script.find("tmux new-session -d -s devdock").expect("tmux");
        assert!(create_at < tmux_at, "log must be created before detach");
        assert!(script.contains(">> '/tmp/devdock-start-abc.log' 2>&1"));
        assert!(script.contains(r"ec=\$?"));
        assert!(
            script.contains("echo $ec > '/tmp/devdock-start-abc.exit'")
                || script.contains(r"echo \$ec > '/tmp/devdock-start-abc.exit'")
        );
        assert!(script.contains("DEVDOCK_LOG_CREATED:1"));
        assert!(!script.contains("tail -f /tmp/devdock-start.log &&"));
    }

    #[test]
    fn start_script_includes_service_filter() {
        let script = build_remote_start_script(
            "export DEVDOCK_ID='abc'",
            "qe",
            Some("api,web"),
            "/tmp/devdock-start-abc.log",
            "/tmp/devdock-start-abc.exit",
            "/tmp/devdock-start-abc.meta",
        );
        assert!(script.contains("devdock start qe -s api,web"));
    }

    #[test]
    fn parse_tmux_dead_with_log_and_exit() {
        let output = "\
DEVDOCK_STATUS:tmux_dead
DEVDOCK_EXIT:17
DEVDOCK_LOG_EXISTS:1
DEVDOCK_LOG_PATH:/tmp/devdock-start-abc.log
DEVDOCK_LOG_TAIL_BEGIN
boom: profile not found
tilt failed
DEVDOCK_LOG_TAIL_END
";
        let diag = parse_tmux_dead_diagnostics(output);
        assert_eq!(diag.exit_status, Some(17));
        assert!(diag.log_exists);
        assert_eq!(diag.log_path.as_deref(), Some("/tmp/devdock-start-abc.log"));
        assert!(diag.log_tail.contains("boom: profile not found"));
        assert!(diag.log_tail.contains("tilt failed"));

        let msg = format_tmux_dead_message("ws-1", "default_profile", &diag);
        assert!(msg.contains("exit status: 17"));
        assert!(msg.contains("/tmp/devdock-start-abc.log"));
        assert!(msg.contains("boom: profile not found"));
        let failed = command_failed_for_tmux_dead(&diag);
        assert!(failed.contains("exit 17"));
        assert!(failed.contains("/tmp/devdock-start-abc.log"));
    }

    #[test]
    fn parse_tmux_dead_without_log_does_not_advertise_path() {
        let output = "\
DEVDOCK_STATUS:tmux_dead
DEVDOCK_EXIT:unknown
DEVDOCK_LOG_EXISTS:0
";
        let diag = parse_tmux_dead_diagnostics(output);
        assert_eq!(diag.exit_status, None);
        assert!(!diag.log_exists);
        assert!(diag.log_path.is_none());

        let msg = format_tmux_dead_message("ws-1", "default_profile", &diag);
        assert!(msg.contains("No remote start log was captured"));
        assert!(!msg.contains("cat /tmp/devdock-start.log"));
        assert!(!msg.contains("tail -f /tmp/devdock-start.log"));
    }

    #[test]
    fn wait_check_script_emits_tmux_dead_markers() {
        let script =
            tilt_wait_check_script("/tmp/devdock-start-abc.log", "/tmp/devdock-start-abc.exit");
        assert!(script.contains("DEVDOCK_STATUS:tmux_dead"));
        assert!(script.contains("DEVDOCK_LOG_TAIL_BEGIN"));
        assert!(script.contains("tail -n 40"));
        let tmux_dead_at = script.find("DEVDOCK_STATUS:tmux_dead").unwrap();
        let tail_at = script.find("DEVDOCK_LOG_TAIL_BEGIN").unwrap();
        assert!(tmux_dead_at < tail_at);
    }

    #[test]
    fn classify_timeout_and_stopped_and_unavailable() {
        assert_eq!(
            classify_ssh_probe_failure(&RemoteError::SshTimeout(10), 10),
            RemoteTiltProbe::TimedOut { timeout_secs: 10 }
        );
        assert_eq!(
            classify_ssh_probe_failure(&RemoteError::SshError("workspace is stopped".into()), 10)
                .label(),
            "workspace stopped"
        );
        assert_eq!(
            classify_ssh_probe_failure(
                &RemoteError::SshError("Failed to get workspace agent".into()),
                10
            )
            .label(),
            "agent unavailable"
        );
    }

    #[test]
    fn probe_skips_ssh_when_workspace_stopped() {
        let probe = probe_remote_tilt_with(
            "my-ws",
            10,
            |name, timeout| {
                assert_eq!(name, "my-ws");
                assert_eq!(timeout, REMOTE_STATUS_CODER_LIST_TIMEOUT_SECS);
                Ok(Some("stopped".into()))
            },
            |_ws, _cmd, _timeout| panic!("SSH must not run when workspace is stopped"),
        );
        assert_eq!(probe, RemoteTiltProbe::WorkspaceStopped);
    }

    #[test]
    fn probe_reports_ssh_timeout() {
        let probe = probe_remote_tilt_with(
            "my-ws",
            10,
            |_name, _timeout| Ok(Some("running".into())),
            |_ws, _cmd, timeout| Err(RemoteError::SshTimeout(timeout)),
        );
        assert_eq!(probe, RemoteTiltProbe::TimedOut { timeout_secs: 10 });
        assert_eq!(probe.label(), "timed out");
    }

    #[test]
    fn probe_reports_tilt_running() {
        let probe = probe_remote_tilt_with(
            "my-ws",
            10,
            |_name, _timeout| Ok(Some("running".into())),
            |_ws, cmd, _timeout| {
                assert!(cmd.contains("lsof -i:10350"));
                Ok("running\n".into())
            },
        );
        assert_eq!(probe, RemoteTiltProbe::TiltRunning);
    }

    #[test]
    fn remote_start_paths_are_session_scoped() {
        let log = remote_start_log_path("dev-abc/def");
        assert_eq!(log, "/tmp/devdock-start-dev-abc-def.log");
        assert!(log.starts_with("/tmp/devdock-start-"));
        assert!(log.ends_with(".log"));
    }
}
