//! Workspace lifecycle management.

use crate::{coder, config_sync, port_forward::PortForwardManager, ssh, RemoteError, Result};
use devdock_state::StateManager;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;
use tracing::info;

const REMOTE_SCIO_PATH: &str = "/home/workspace/scio";
const DEFAULT_REMOTE_DEVDOCK_BIN: &str = "devdock";

/// Git state for the repository inside an existing remote workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteGitState {
    pub branch: String,
    pub head_oid: String,
    pub dirty_entries: Vec<String>,
}

/// Bounded, read-only evidence about the remote DevDock runtime.
///
/// This is intentionally independent from local SQLite state. A stopped local
/// session must not treat healthy Docker containers as a current test target
/// until the remote tmux/Tilt/session identity has been proven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRuntimeSnapshot {
    pub tmux_present: bool,
    pub tilt_listening: bool,
    pub docker_containers: Vec<String>,
    pub devdock_id: Option<String>,
    pub git_revision: Option<String>,
    pub git_branch: Option<String>,
}

impl RemoteRuntimeSnapshot {
    pub fn has_surviving_containers(&self) -> bool {
        !self.docker_containers.is_empty()
    }

    pub fn matches_session(&self, instance_id: &str) -> bool {
        self.tmux_present && self.tilt_listening && self.devdock_id.as_deref() == Some(instance_id)
    }
}

/// Inspect the remote tmux/Tilt identity, Docker services, and Git revision.
///
/// The probe is best-effort at the call site: an unavailable workspace is not
/// itself evidence that containers belong to the current run.
pub fn inspect_remote_runtime(
    workspace: &str,
    log_file: Option<&Path>,
) -> Result<RemoteRuntimeSnapshot> {
    let script = r#"
if tmux has-session -t devdock 2>/dev/null; then
    printf 'DEVDOCK_RUNTIME_TMUX\t1\n'
    tmux_id="$(tmux show-environment -t devdock DEVDOCK_ID 2>/dev/null | sed -n 's/^DEVDOCK_ID=//p' | head -n 1)"
else
    printf 'DEVDOCK_RUNTIME_TMUX\t0\n'
    tmux_id=""
fi
printf 'DEVDOCK_RUNTIME_ID\t%s\n' "$tmux_id"
if (ss -tln 2>/dev/null | grep -q ':10350 ' || lsof -nP -iTCP:10350 -sTCP:LISTEN >/dev/null 2>&1); then
    printf 'DEVDOCK_RUNTIME_TILT\t1\n'
else
    printf 'DEVDOCK_RUNTIME_TILT\t0\n'
fi
printf 'DEVDOCK_RUNTIME_CONTAINERS_BEGIN\n'
docker ps --filter 'name=devdock-' --format '{{.Names}}' 2>/dev/null || true
printf 'DEVDOCK_RUNTIME_CONTAINERS_END\n'
git -C /home/workspace/scio rev-parse HEAD 2>/dev/null | sed 's/^/DEVDOCK_RUNTIME_GIT\t/' || true
git -C /home/workspace/scio symbolic-ref --quiet --short HEAD 2>/dev/null | sed 's/^/DEVDOCK_RUNTIME_GIT_BRANCH\t/' || true
"#;
    let output = ssh::ssh_exec_with_log(workspace, script, log_file)?;
    parse_remote_runtime_snapshot(&output)
}

fn parse_remote_runtime_snapshot(output: &str) -> Result<RemoteRuntimeSnapshot> {
    let mut tmux_present = false;
    let mut tilt_listening = false;
    let mut devdock_id = None;
    let mut git_revision = None;
    let mut git_branch = None;
    let mut docker_containers = Vec::new();
    let mut in_containers = false;

    for line in output.lines() {
        if line == "DEVDOCK_RUNTIME_CONTAINERS_BEGIN" {
            in_containers = true;
            continue;
        }
        if line == "DEVDOCK_RUNTIME_CONTAINERS_END" {
            in_containers = false;
            continue;
        }
        if in_containers {
            let container = line.trim();
            if !container.is_empty() {
                docker_containers.push(container.to_string());
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("DEVDOCK_RUNTIME_TMUX\t") {
            tmux_present = value.trim() == "1";
        } else if let Some(value) = line.strip_prefix("DEVDOCK_RUNTIME_TILT\t") {
            tilt_listening = value.trim() == "1";
        } else if let Some(value) = line.strip_prefix("DEVDOCK_RUNTIME_ID\t") {
            devdock_id = non_empty_runtime_value(value);
        } else if let Some(value) = line.strip_prefix("DEVDOCK_RUNTIME_GIT\t") {
            git_revision = non_empty_runtime_value(value);
        } else if let Some(value) = line.strip_prefix("DEVDOCK_RUNTIME_GIT_BRANCH\t") {
            git_branch = non_empty_runtime_value(value);
        }
    }

    Ok(RemoteRuntimeSnapshot {
        tmux_present,
        tilt_listening,
        docker_containers,
        devdock_id,
        git_revision,
        git_branch,
    })
}

fn non_empty_runtime_value(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

impl RemoteGitState {
    pub fn is_dirty(&self) -> bool {
        !self.dirty_entries.is_empty()
    }
}

/// Inspect the git branch, exact HEAD, and dirty state of the remote repository.
pub fn inspect_remote_git_state(
    workspace: &str,
    log_file: Option<&Path>,
) -> Result<RemoteGitState> {
    inspect_remote_git_state_with(workspace, log_file, ssh::ssh_exec_with_log)
}

fn inspect_remote_git_state_with<F>(
    workspace: &str,
    log_file: Option<&Path>,
    mut exec: F,
) -> Result<RemoteGitState>
where
    F: FnMut(&str, &str, Option<&Path>) -> Result<String>,
{
    let script = format!(
        r#"
set -e
cd {REMOTE_SCIO_PATH}
branch="$(git symbolic-ref --quiet --short HEAD || printf '<detached>')"
head_oid="$(git rev-parse --verify HEAD)"
printf 'DEVDOCK_GIT_BRANCH\t%s\n' "$branch"
printf 'DEVDOCK_GIT_HEAD\t%s\n' "$head_oid"
printf 'DEVDOCK_GIT_STATUS_BEGIN\n'
git status --porcelain=v1 --untracked-files=all
printf 'DEVDOCK_GIT_STATUS_END\n'
"#
    );

    let output = exec(workspace, &script, log_file)?;
    parse_remote_git_state(&output)
}

fn parse_remote_git_state(output: &str) -> Result<RemoteGitState> {
    let mut branch = None;
    let mut head_oid = None;
    let mut dirty_entries = Vec::new();
    let mut in_status = false;

    for line in output.lines() {
        if let Some(value) = line.strip_prefix("DEVDOCK_GIT_BRANCH\t") {
            branch = Some(value.trim().to_string());
            continue;
        }

        if let Some(value) = line.strip_prefix("DEVDOCK_GIT_HEAD\t") {
            head_oid = Some(value.trim().to_string());
            continue;
        }

        if line == "DEVDOCK_GIT_STATUS_BEGIN" {
            in_status = true;
            continue;
        }

        if line == "DEVDOCK_GIT_STATUS_END" {
            in_status = false;
            continue;
        }

        if in_status && !line.trim().is_empty() {
            dirty_entries.push(line.to_string());
        }
    }

    let branch = branch.filter(|b| !b.is_empty()).ok_or_else(|| {
        RemoteError::CommandFailed("Failed to parse remote git branch".to_string())
    })?;
    let head_oid = head_oid.filter(|oid| is_full_git_oid(oid)).ok_or_else(|| {
        RemoteError::CommandFailed("Failed to parse remote git HEAD OID".to_string())
    })?;

    Ok(RemoteGitState { branch, head_oid, dirty_entries })
}

/// Converge an owned, clean checkout to one immutable local source OID.
///
/// This is intentionally a fetch + checkout/reset sequence, rather than a
/// merge, rebase, or `git pull`, so a force-pushed branch is safe to reuse.
/// The remote shell rechecks cleanliness immediately before mutating Git.
pub fn reconcile_remote_checkout(
    workspace: &str,
    branch: &str,
    expected_oid: &str,
    managed_residue_path: Option<&str>,
    log_file: Option<&Path>,
) -> Result<RemoteGitState> {
    if !is_valid_branch_name(branch) {
        return Err(RemoteError::RemoteCheckoutUnsafe(format!(
            "invalid requested branch '{branch}'"
        )));
    }
    if !is_full_git_oid(expected_oid) {
        return Err(RemoteError::RemoteCheckoutUnsafe(format!(
            "requested OID for branch '{branch}' is not a full SHA-1/SHA-256 Git OID"
        )));
    }
    if let Some(path) = managed_residue_path {
        if !is_valid_repo_relative_path(path) {
            return Err(RemoteError::RemoteCheckoutUnsafe(format!(
                "managed residue path is not a safe repository-relative path: '{path}'"
            )));
        }
    }

    let script = remote_checkout_script(branch, expected_oid, managed_residue_path);
    let output = ssh::ssh_exec_with_log(workspace, &script, log_file)?;
    let state = parse_remote_git_state(&output)?;
    if state.branch != branch || state.head_oid != expected_oid || state.is_dirty() {
        return Err(RemoteError::RemoteCheckoutUnsafe(format!(
            "post-checkout state does not match requested branch/OID (branch={}, head={}, dirty={})",
            state.branch,
            state.head_oid,
            state.is_dirty()
        )));
    }

    info!(branch, expected_oid, "Reconciled remote checkout to requested local source OID");
    Ok(state)
}

fn remote_checkout_script(
    branch: &str,
    expected_oid: &str,
    managed_residue_path: Option<&str>,
) -> String {
    let branch_ref = format!("refs/heads/{branch}");
    let origin_ref = format!("refs/remotes/origin/{branch}");
    let refspec = format!("+{branch_ref}:{origin_ref}");
    let branch = shell_quote(branch);
    let refspec = shell_quote(&refspec);
    let expected_oid = shell_quote(expected_oid);
    let pre_fetch_dirty_guard = managed_residue_path.map_or_else(
        || {
            r#"if test -n "$(git status --porcelain=v1 --untracked-files=all)"; then
    echo 'DEVDOCK_CHECKOUT_DIRTY'
    exit 70
fi"#
            .to_string()
        },
        |path| {
            let path = shell_quote(path);
            format!(
                r#"managed_path={path}
dirty_status="$(git status --porcelain=v1 --untracked-files=all)"
if test -n "$dirty_status"; then
    if test "$dirty_status" != " M $managed_path"; then
        echo 'DEVDOCK_CHECKOUT_DIRTY'
        exit 70
    fi
    git checkout -- "$managed_path"
fi
if test -n "$(git status --porcelain=v1 --untracked-files=all)"; then
    echo 'DEVDOCK_CHECKOUT_DIRTY'
    exit 70
fi"#
            )
        },
    );

    format!(
        r#"set -eu
cd {REMOTE_SCIO_PATH}
{pre_fetch_dirty_guard}
git check-ref-format --branch {branch} >/dev/null
git fetch --no-tags --force origin {refspec}
if test -n "$(git status --porcelain=v1 --untracked-files=all)"; then
    echo 'DEVDOCK_CHECKOUT_DIRTY'
    exit 70
fi
if ! git rev-parse --verify {expected_oid}^{{commit}} >/dev/null 2>&1; then
    echo 'DEVDOCK_CHECKOUT_OID_MISMATCH'
    exit 71
fi
git checkout --force -B {branch} {expected_oid}
git reset --hard {expected_oid} >/dev/null
if test -n "$(git status --porcelain=v1 --untracked-files=all)"; then
    echo 'DEVDOCK_CHECKOUT_POST_DIRTY'
    exit 72
fi
current_branch="$(git symbolic-ref --quiet --short HEAD)"
current_oid="$(git rev-parse --verify HEAD)"
if test "$current_branch" != {branch} || test "$current_oid" != {expected_oid}; then
    echo 'DEVDOCK_CHECKOUT_POST_MISMATCH'
    exit 73
fi
printf 'DEVDOCK_GIT_BRANCH\t%s\n' "$current_branch"
printf 'DEVDOCK_GIT_HEAD\t%s\n' "$current_oid"
printf 'DEVDOCK_GIT_STATUS_BEGIN\n'
git status --porcelain=v1 --untracked-files=all
printf 'DEVDOCK_GIT_STATUS_END\n'
"#
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Select the binary that the Coder workspace's tmux session executes.
///
/// `DEVDOCK_REMOTE_BIN` is deliberately distinct from a host-side candidate
/// selection: it names a run-owned executable that already exists inside the
/// remote workspace. Leaving it unset retains the production `devdock` lookup.
fn configured_remote_devdock_bin() -> String {
    let configured = std::env::var("DEVDOCK_REMOTE_BIN").ok();
    select_remote_devdock_bin(configured.as_deref())
}

fn select_remote_devdock_bin(configured: Option<&str>) -> String {
    configured
        .filter(|path| !path.trim().is_empty())
        .unwrap_or(DEFAULT_REMOTE_DEVDOCK_BIN)
        .to_string()
}

fn is_valid_branch_name(branch: &str) -> bool {
    !branch.is_empty()
        && !branch.starts_with('-')
        && !branch.starts_with('.')
        && !branch.ends_with('.')
        && !branch.contains("..")
        && !branch.contains("@{")
        && !branch.ends_with('/')
        && branch.bytes().all(|byte| !byte.is_ascii_control() && !b" ~^:?*[\\".contains(&byte))
}

fn is_full_git_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_valid_repo_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.split('/').any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
        && path.bytes().all(|byte| !byte.is_ascii_control())
}

/// Provision a new Coder workspace.
pub async fn provision_workspace(config: &coder::CoderWorkspaceConfig) -> Result<String> {
    info!("Provisioning workspace: {}", config.workspace_name);

    // Create workspace
    let workspace_name = coder::create_workspace(config)?;

    // Wait for SSH to be ready
    ssh::wait_for_ssh_ready(&workspace_name, 300)?; // 5 minute timeout

    Ok(workspace_name)
}

/// Sync configuration and credentials before the initial source-sync watermark.
///
/// The engine keeps ordinary profile/custom.ini transfer, but delegates every
/// gcloud credential byte to Scio's existing helper. No legacy DevDock gcloud
/// staging/finalization fallback is available on this path.
pub fn sync_remote_configuration(
    workspace: &str,
    profile_path: &Path,
    profile_project: &str,
    copy_git_creds: bool,
    log_file: Option<&Path>,
    verbose: bool,
) -> Result<()> {
    let custom_ini_path = find_custom_ini_path();
    config_sync::sync_all_config(
        workspace,
        profile_path,
        custom_ini_path.as_deref(),
        copy_git_creds,
        log_file,
        verbose,
    )?;

    let platform = config_sync::get_profile_platform(profile_path)?;
    if matches!(platform.as_str(), "google" | "gcp") {
        config_sync::sync_gcloud_credentials_via_scio_helper(
            workspace,
            profile_project,
            log_file,
            verbose,
        )?;
    }
    Ok(())
}

/// Start DevDock on remote workspace with optional port forwarding.
#[allow(clippy::too_many_arguments)]
pub async fn start_remote_devdock(
    state: &StateManager,
    workspace: &str,
    workspace_id: &str,
    profile: &str,
    profile_path: &Path,
    devdock_id: &str,
    services: Option<&str>,
    auto_forward: bool,
    offset: u16,
    log_file: Option<&Path>,
    verbose: bool,
) -> Result<Option<PortForwardManager>> {
    info!("Starting DevDock on remote workspace: {}", workspace);

    // Log start
    if let Some(log_path) = log_file {
        use std::fs::OpenOptions;
        use std::io::Write;
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
            let _ = writeln!(file, "\n=== Starting DevDock on Remote Workspace ===");
            let _ = writeln!(file, "Workspace: {workspace}");
            let _ = writeln!(file, "Profile: {profile}");
            let _ = writeln!(file, "DevDock ID: {devdock_id}");
            let _ = writeln!(file, "Port Offset: {offset}");
            let _ = writeln!(file, "Services: {services:?}");
        }
    }

    // Start DevDock in tmux session on remote only after the caller has
    // established an initial source-sync watermark.
    // Build only non-secret environment exports. In particular, never interpolate
    // local credential values into an SSH command because operation logging would
    // retain that command.
    let env_exports = format!(
        "export DEVDOCK_ID={} && export DEVDOCK_PORT_OFFSET={offset}",
        shell_quote(devdock_id)
    );

    let runtime = inspect_remote_runtime(workspace, log_file)?;
    if runtime.tmux_present {
        if runtime.devdock_id.as_deref() != Some(devdock_id) {
            return Err(RemoteError::CommandFailed(
                "refusing to replace a remote DevDock tmux session whose DEVDOCK_ID is not owned by this attempt"
                    .to_string(),
            ));
        }
        ssh::ssh_exec_with_log(workspace, &owned_tmux_stop_script(devdock_id), log_file)?;
        sleep(Duration::from_secs(1));
    } else if runtime.tilt_listening {
        return Err(RemoteError::CommandFailed(
            "refusing to reuse a Tilt listener without an owned DevDock tmux session".to_string(),
        ));
    }

    let remote_devdock_bin = configured_remote_devdock_bin();
    let start_script = remote_start_script(
        profile,
        services,
        &env_exports,
        &remote_devdock_bin,
        devdock_id,
        offset,
    );

    info!("Starting DevDock in tmux session");
    if verbose {
        println!("  Starting DevDock on remote workspace...");
        println!("  Profile: {profile}");
        println!("  Remote DevDock binary: {remote_devdock_bin}");
        println!("  DevDock ID: {}", &devdock_id[..8.min(devdock_id.len())]);
        println!("  Port Offset: {offset}");
    }

    ssh::ssh_exec_with_log(workspace, &start_script, log_file)?;
    if verbose {
        println!("  ✓ DevDock start command executed");
    }

    if verbose {
        println!("  ✓ DevDock command started in background");
        println!("  To view logs: coder ssh {workspace} -- tmux attach -t devdock");
    }

    // 3. Wait for Tilt process to start
    // Combined check: verifies tmux is alive AND Tilt port 10350 is listening
    // in a single SSH call per iteration (replaces 2 separate SSH calls + sleep(5))
    if verbose {
        println!("  Waiting for Tilt to start (timeout: 60s)...");
    }
    let timeout = Duration::from_secs(60);
    let start = std::time::Instant::now();
    let mut last_print = 0u64;

    // Combined tmux + port check script (single SSH call per iteration)
    // Always exits 0 so ssh_exec_with_log returns Ok; status is in stdout markers
    let combined_check = r#"
if ! tmux has-session -t devdock 2>/dev/null; then
    echo "DEVDOCK_STATUS:tmux_dead"
    exit 0
fi
if ss -tlnp 2>/dev/null | grep -q ':10350 ' 2>/dev/null; then
    echo "DEVDOCK_STATUS:tilt_ready"
    exit 0
fi
echo "DEVDOCK_STATUS:waiting"
exit 0
"#;

    loop {
        let elapsed = start.elapsed().as_secs();

        match ssh::ssh_exec_with_log(workspace, combined_check, log_file) {
            Ok(output) if output.contains("DEVDOCK_STATUS:tilt_ready") => {
                if verbose {
                    println!("  ✓ Tilt process is running on port 10350!");
                }
                break;
            }
            Ok(output) if output.contains("DEVDOCK_STATUS:tmux_dead") => {
                eprintln!("\n  ❌ ERROR: DevDock tmux session died while waiting for Tilt!");
                eprintln!("\n  To debug:");
                eprintln!("    1. SSH to workspace: coder ssh {workspace}");
                eprintln!("    2. Check logs: cat /tmp/devdock-start.log");
                eprintln!("    3. Try manual start: devdock start {profile}");
                return Err(RemoteError::CommandFailed(
                    "DevDock process died during startup".to_string(),
                ));
            }
            Ok(_) => {
                // DEVDOCK_STATUS:waiting or unknown — not ready yet, continue
            }
            Err(_) => {
                // SSH connection error — transient, keep waiting
            }
        }

        // Print progress every 10 seconds
        if elapsed > 0 && elapsed >= last_print + 10 {
            println!("    ... still waiting ({elapsed}s elapsed)");
            last_print = elapsed;
        }

        if start.elapsed() > timeout {
            return Err(RemoteError::CommandFailed(format!(
                "Tilt did not listen on port 10350 after {} seconds",
                timeout.as_secs()
            )));
        }

        sleep(Duration::from_secs(2));
    }

    if verbose {
        println!();
        println!("  To view DevDock logs:");
        println!("    coder ssh {workspace}");
        println!("    tmux attach -t devdock");
    }

    // 4. Setup port forwarding
    if verbose {
        println!();
        println!("  Setting up port forwarding...");
    }
    let port_mgr = if auto_forward {
        info!("Setting up auto port forwarding...");
        let mut mgr = PortForwardManager::from_profile(
            profile_path,
            workspace,
            workspace_id,
            devdock_id,
            offset,
        )?;
        mgr.start_all_forwards(state).await?;
        if verbose {
            println!("  ✓ Auto port forwarding enabled");
        }
        Some(mgr)
    } else {
        // Compatibility mode: forward Tilt UI and nginx so local.glean.com routes work.
        info!("Forwarding Tilt UI and nginx (use --auto-forward for all services)");
        let tilt_port = offset + 10350;
        println!("  Starting port forward: localhost:{tilt_port} -> remote:10350");
        println!("  Starting port forward: localhost:8321 -> remote:8321 (nginx)");
        let mut mgr =
            PortForwardManager::default_forwards(workspace, workspace_id, devdock_id, offset);
        mgr.start_all_forwards(state).await?;

        if verbose {
            println!("  ✓ Tilt UI and nginx port forwards started");
        }
        Some(mgr)
    };

    Ok(port_mgr)
}

/// Quote each DevDock argument, then quote the entire command passed as the
/// final `tmux new-session` argument. This preserves argument boundaries across
/// both shells and prevents profile/service text from becoming shell syntax.
fn remote_start_script(
    profile: &str,
    services: Option<&str>,
    env_exports: &str,
    remote_devdock_bin: &str,
    devdock_id: &str,
    port_offset: u16,
) -> String {
    let mut devdock_command =
        format!("{} start {}", shell_quote(remote_devdock_bin), shell_quote(profile));
    if let Some(services) = services {
        devdock_command.push_str(" -s ");
        devdock_command.push_str(&shell_quote(services));
    }
    // The redirect is inside the tmux command, so only remote DevDock output
    // reaches the deterministic diagnostic log. `>` deliberately truncates a
    // prior attempt's log rather than mixing evidence from separate attempts.
    devdock_command.push_str(" > ");
    devdock_command.push_str(&shell_quote("/tmp/devdock-start.log"));
    devdock_command.push_str(" 2>&1");

    let tmux_devdock_id = shell_quote(&format!("DEVDOCK_ID={devdock_id}"));
    let tmux_port_offset = shell_quote(&format!("DEVDOCK_PORT_OFFSET={port_offset}"));
    format!(
        "{env_exports} && cd /home/workspace/scio && tmux new-session -d -s devdock -e {tmux_devdock_id} -e {tmux_port_offset} {}",
        shell_quote(&devdock_command)
    )
}

/// Recheck ownership in the same remote command that terminates a prior tmux
/// session, closing the race between the read-only runtime inspection and kill.
fn owned_tmux_stop_script(devdock_id: &str) -> String {
    let expected_environment = shell_quote(&format!("DEVDOCK_ID={devdock_id}"));
    format!(
        r#"set -eu
observed="$(tmux show-environment -t devdock DEVDOCK_ID 2>/dev/null || true)"
if test "$observed" != {expected_environment}; then
    echo 'DEVDOCK_OWNERSHIP_MISMATCH'
    exit 74
fi
tmux kill-session -t devdock
"#
    )
}

/// Wait for Tilt UI to become accessible with progress updates.
pub fn wait_for_tilt_ui(port: u16, timeout_secs: u64) -> Result<()> {
    use std::time::Instant;

    info!("Waiting for Tilt UI at http://localhost:{}", port);
    println!("  Waiting for Tilt UI to become ready (this can take 3-5 minutes)...");
    println!("  Check logs on remote: coder ssh <workspace> and look at tmux or DevDock logs");

    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let mut last_update = 0u64;

    loop {
        let elapsed = start.elapsed().as_secs();

        // Print progress every 10 seconds
        if elapsed - last_update >= 10 {
            println!(
                "  Still waiting... ({}s elapsed, timeout in {}s)",
                elapsed,
                timeout_secs - elapsed
            );
            last_update = elapsed;
        }

        // Try to connect to Tilt UI
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            info!("Tilt UI is ready");
            println!("  ✓ Tilt UI is ready!");
            return Ok(());
        }

        if start.elapsed() > timeout {
            eprintln!("\n  ❌ Timeout: Tilt UI not ready after {timeout_secs} seconds");
            eprintln!("  Debug steps:");
            eprintln!("    1. SSH into workspace: coder ssh <workspace-name>");
            eprintln!("    2. Check if DevDock started: ps aux | grep devdock");
            eprintln!("    3. Check Tilt: ps aux | grep tilt");
            eprintln!("    4. View logs: journalctl --user -f");
            return Err(RemoteError::CommandFailed(format!(
                "Tilt UI not ready after {timeout_secs} seconds"
            )));
        }

        sleep(Duration::from_secs(5));
    }
}

/// Resolve config/custom.ini from workspace root, if it exists.
/// This file is typically .gitignore'd but still needs to be synced to remote.
fn find_custom_ini_path() -> Option<PathBuf> {
    let devdock_root = std::env::var("DEVDOCK_ROOT")
        .or_else(|_| std::env::var("PWD"))
        .unwrap_or_else(|_| ".".to_string());
    let path = PathBuf::from(devdock_root).join("config/custom.ini");
    path.exists().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_remote_git_state_clean() {
        let state = parse_remote_git_state(
            "DEVDOCK_GIT_BRANCH\tfeature/foo\nDEVDOCK_GIT_HEAD\t0123456789abcdef0123456789abcdef01234567\nDEVDOCK_GIT_STATUS_BEGIN\nDEVDOCK_GIT_STATUS_END\n",
        )
        .expect("parse clean state");

        assert_eq!(state.branch, "feature/foo");
        assert_eq!(state.head_oid, "0123456789abcdef0123456789abcdef01234567");
        assert!(state.dirty_entries.is_empty());
        assert!(!state.is_dirty());
    }

    #[test]
    fn parse_remote_git_state_dirty() {
        let state = parse_remote_git_state(
            "DEVDOCK_GIT_BRANCH\tfeature/foo\nDEVDOCK_GIT_HEAD\t0123456789abcdef0123456789abcdef01234567\nDEVDOCK_GIT_STATUS_BEGIN\n M java/Foo.java\n?? python_scio/new_file.py\nR  old.java -> new.java\nDEVDOCK_GIT_STATUS_END\n",
        )
        .expect("parse dirty state");

        assert_eq!(state.branch, "feature/foo");
        assert_eq!(
            state.dirty_entries,
            vec![
                " M java/Foo.java".to_string(),
                "?? python_scio/new_file.py".to_string(),
                "R  old.java -> new.java".to_string(),
            ]
        );
        assert!(state.is_dirty());
    }

    #[test]
    fn parse_remote_git_state_requires_branch() {
        let err = parse_remote_git_state(
            "DEVDOCK_GIT_HEAD\t0123456789abcdef0123456789abcdef01234567\nDEVDOCK_GIT_STATUS_BEGIN\nDEVDOCK_GIT_STATUS_END\n",
        )
        .expect_err("missing branch should fail");

        assert!(matches!(err, RemoteError::CommandFailed(_)));
    }

    #[test]
    fn inspect_remote_git_state_with_uses_injected_executor() {
        let state =
            inspect_remote_git_state_with("ws-name", None, |workspace, command, log_file| {
                assert_eq!(workspace, "ws-name");
                assert!(command.contains("cd /home/workspace/scio"));
                assert!(command.contains("git status --porcelain=v1"));
                assert!(log_file.is_none());
                Ok("DEVDOCK_GIT_BRANCH\tmain\nDEVDOCK_GIT_HEAD\t0123456789abcdef0123456789abcdef01234567\nDEVDOCK_GIT_STATUS_BEGIN\nDEVDOCK_GIT_STATUS_END\n"
                    .to_string())
            })
            .expect("inspect with injected executor");

        assert_eq!(state.branch, "main");
        assert!(state.dirty_entries.is_empty());
    }

    #[test]
    fn checkout_script_fetches_the_named_branch_and_resets_to_the_exact_oid() {
        let oid = "0123456789abcdef0123456789abcdef01234567";
        let script = remote_checkout_script("feature/reused", oid, None);

        assert!(script.contains("git fetch --no-tags --force origin '+refs/heads/feature/reused:refs/remotes/origin/feature/reused'"));
        assert!(script.contains(
            "git checkout --force -B 'feature/reused' '0123456789abcdef0123456789abcdef01234567'"
        ));
        assert!(script.contains("git reset --hard '0123456789abcdef0123456789abcdef01234567'"));
        assert!(!script.contains("git pull"));
        assert!(script.contains("git status --porcelain=v1 --untracked-files=all"));
    }

    #[test]
    fn checkout_script_allows_a_reachable_local_commit_after_origin_advances() {
        let oid = "0123456789abcdef0123456789abcdef01234567";
        let script = remote_checkout_script("feature/reused", oid, None);

        assert!(script.contains("git fetch --no-tags --force origin '+refs/heads/feature/reused:refs/remotes/origin/feature/reused'"));
        assert!(script.contains(
            "git rev-parse --verify '0123456789abcdef0123456789abcdef01234567'^{commit} >/dev/null 2>&1"
        ));
        assert!(!script.contains("observed_oid="));
        assert!(!script.contains("origin/feature/reused'^{commit}"));
    }

    #[test]
    fn checkout_script_rechecks_cleanliness_after_fetch_before_resetting() {
        let script = remote_checkout_script(
            "feature/reused",
            "0123456789abcdef0123456789abcdef01234567",
            None,
        );
        let fetch = script.find("git fetch --no-tags --force origin").expect("fetch command");
        let post_fetch_guard = script[fetch..]
            .find("DEVDOCK_CHECKOUT_DIRTY")
            .map(|offset| fetch + offset)
            .expect("post-fetch dirty guard");
        let checkout = script.find("git checkout --force -B").expect("checkout command");

        assert!(post_fetch_guard < checkout, "dirty changes after fetch must stop reset");
        assert_eq!(script.matches("DEVDOCK_CHECKOUT_DIRTY").count(), 2);
        assert!(!script.contains("git stash"));
    }

    #[test]
    fn checkout_script_quotes_branch_names_without_interpolation() {
        let script = remote_checkout_script(
            "feature/o'hare",
            "0123456789abcdef0123456789abcdef01234567",
            None,
        );
        assert!(script.contains("'feature/o'\"'\"'hare'"));
    }

    #[test]
    fn checkout_script_reconciles_only_an_exact_unstaged_managed_profile_residue() {
        let managed_path = "devdock/profiles/qe_smoke_test.yaml";
        let script = remote_checkout_script(
            "feature/reused",
            "0123456789abcdef0123456789abcdef01234567",
            Some(managed_path),
        );
        let restore = script.find("git checkout -- \"$managed_path\"").expect("path restore");
        let fetch = script.find("git fetch --no-tags --force origin").expect("fetch command");

        assert!(script.contains("managed_path='devdock/profiles/qe_smoke_test.yaml'"));
        assert!(script.contains("if test \"$dirty_status\" != \" M $managed_path\"; then"));
        assert!(restore < fetch, "managed profile residue must be restored before fetch");
        assert!(!script.contains("git stash"));
    }

    #[test]
    fn remote_start_script_quotes_untrusted_profile_and_services_for_tmux() {
        let profile = "profile'; touch /tmp/escaped; #";
        let services = "qe; $(touch /tmp/escaped) && #";
        let env_exports = "export DEVDOCK_ID='owned' && export DEVDOCK_PORT_OFFSET=10350";
        let remote_devdock_bin = "/run/devdock-candidate";
        let inner_command = format!(
            "{} start {} -s {} > {} 2>&1",
            shell_quote(remote_devdock_bin),
            shell_quote(profile),
            shell_quote(services),
            shell_quote("/tmp/devdock-start.log")
        );
        let script = remote_start_script(
            profile,
            Some(services),
            env_exports,
            remote_devdock_bin,
            "owned",
            10350,
        );
        let tmux_prefix = format!(
            "{env_exports} && cd /home/workspace/scio && tmux new-session -d -s devdock -e 'DEVDOCK_ID=owned' -e 'DEVDOCK_PORT_OFFSET=10350' "
        );
        let tmux_argument = script.strip_prefix(&tmux_prefix).expect("tmux command argument");

        assert_eq!(tmux_argument, shell_quote(&inner_command));
        assert_ne!(tmux_argument, inner_command);
        assert!(inner_command.ends_with(" > '/tmp/devdock-start.log' 2>&1"));
        assert!(!inner_command.contains(">>"));
        assert!(!script.contains(&format!("'{remote_devdock_bin} start {profile} -s {services}'")));
    }

    #[test]
    fn remote_candidate_bin_defaults_to_production_and_ignores_blank_override() {
        assert_eq!(select_remote_devdock_bin(None), "devdock");
        assert_eq!(select_remote_devdock_bin(Some("  \t")), "devdock");
        assert_eq!(
            select_remote_devdock_bin(Some("/run/devdock-candidate")),
            "/run/devdock-candidate"
        );
    }

    #[test]
    fn remote_start_script_quotes_explicit_remote_candidate_path() {
        let remote_devdock_bin = "/run/candidate'; touch /tmp/escaped; #";
        let env_exports = "export DEVDOCK_ID='owned' && export DEVDOCK_PORT_OFFSET=10350";
        let inner_command = format!(
            "{} start {} > {} 2>&1",
            shell_quote(remote_devdock_bin),
            shell_quote("qe"),
            shell_quote("/tmp/devdock-start.log")
        );
        let script =
            remote_start_script("qe", None, env_exports, remote_devdock_bin, "owned", 10350);
        let tmux_prefix = format!(
            "{env_exports} && cd /home/workspace/scio && tmux new-session -d -s devdock -e 'DEVDOCK_ID=owned' -e 'DEVDOCK_PORT_OFFSET=10350' "
        );
        let tmux_argument = script.strip_prefix(&tmux_prefix).expect("tmux command argument");

        assert_eq!(tmux_argument, shell_quote(&inner_command));
        assert!(!script.contains(&format!("tmux new-session -d -s devdock {remote_devdock_bin}")));
    }

    #[test]
    fn owned_tmux_stop_rechecks_the_exact_devdock_id() {
        let script = owned_tmux_stop_script("dev-550e8400-e29b-41d4-a716-446655440000");

        assert!(script.contains("tmux show-environment -t devdock DEVDOCK_ID"));
        assert!(script.contains("DEVDOCK_ID=dev-550e8400-e29b-41d4-a716-446655440000"));
        assert!(script.contains("tmux kill-session -t devdock"));
        assert!(!script.contains("pkill"));
    }

    #[test]
    fn validates_branch_names_and_full_oids_before_remote_mutation() {
        assert!(is_valid_branch_name("feature/reuse-123"));
        assert!(!is_valid_branch_name("feature/../escape"));
        assert!(!is_valid_branch_name("-option"));
        assert!(!is_valid_branch_name("feature name"));
        assert!(is_valid_repo_relative_path("devdock/profiles/qe_smoke_test.yaml"));
        assert!(!is_valid_repo_relative_path("../outside"));
        assert!(!is_valid_repo_relative_path("devdock//profiles/qe.yaml"));
        assert!(is_full_git_oid("0123456789abcdef0123456789abcdef01234567"));
        assert!(is_full_git_oid(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!is_full_git_oid("01234567"));
    }

    #[test]
    fn parses_remote_runtime_snapshot_with_containers_and_identity() {
        let output = "DEVDOCK_RUNTIME_TMUX\t0\nDEVDOCK_RUNTIME_ID\tdev-session\nDEVDOCK_RUNTIME_TILT\t0\nDEVDOCK_RUNTIME_CONTAINERS_BEGIN\ndevdock-qe\ndevdock-nginx\nDEVDOCK_RUNTIME_CONTAINERS_END\nDEVDOCK_RUNTIME_GIT\t0123456789abcdef\nDEVDOCK_RUNTIME_GIT_BRANCH\tmain\n";

        let snapshot = parse_remote_runtime_snapshot(output).unwrap();

        assert!(!snapshot.tmux_present);
        assert!(!snapshot.tilt_listening);
        assert_eq!(snapshot.devdock_id.as_deref(), Some("dev-session"));
        assert_eq!(snapshot.git_revision.as_deref(), Some("0123456789abcdef"));
        assert_eq!(snapshot.git_branch.as_deref(), Some("main"));
        assert_eq!(snapshot.docker_containers, vec!["devdock-qe", "devdock-nginx"]);
        assert!(snapshot.has_surviving_containers());
        assert!(!snapshot.matches_session("dev-session"));
    }

    #[test]
    fn runtime_snapshot_requires_exact_session_identity() {
        let snapshot = RemoteRuntimeSnapshot {
            tmux_present: true,
            tilt_listening: true,
            docker_containers: vec!["devdock-qe".to_string()],
            devdock_id: Some("expected".to_string()),
            git_revision: None,
            git_branch: None,
        };

        assert!(snapshot.matches_session("expected"));
        assert!(!snapshot.matches_session("other"));
    }
}
