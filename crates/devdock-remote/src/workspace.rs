//! Workspace lifecycle management.

use crate::{coder, config_sync, diag, port_forward::PortForwardManager, ssh, RemoteError, Result};
use devdock_state::StateManager;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;
use tracing::info;

const REMOTE_SCIO_PATH: &str = "/home/workspace/scio";

/// Git state for the repository inside an existing remote workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteGitState {
    pub branch: String,
    pub dirty_entries: Vec<String>,
}

impl RemoteGitState {
    pub fn is_dirty(&self) -> bool {
        !self.dirty_entries.is_empty()
    }
}

/// Inspect the git branch and dirty state of the remote repository.
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
branch="$(git symbolic-ref --quiet --short HEAD || git rev-parse --short HEAD)"
printf 'DEVDOCK_GIT_BRANCH\t%s\n' "$branch"
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
    let mut dirty_entries = Vec::new();
    let mut in_status = false;

    for line in output.lines() {
        if let Some(value) = line.strip_prefix("DEVDOCK_GIT_BRANCH\t") {
            branch = Some(value.trim().to_string());
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

    Ok(RemoteGitState { branch, dirty_entries })
}

/// Cleanup existing DevDock session on remote workspace.
pub fn cleanup_existing_devdock(workspace: &str, log_file: Option<&Path>) -> Result<()> {
    info!("Cleaning up existing DevDock processes on {}", workspace);

    // Optimized cleanup: Single SSH session with parallelized independent operations
    // This reduces 5 sequential SSH calls (~10 seconds) to 1 batched call (~2 seconds)
    let cleanup_script = r#"
# Parallelize independent cleanup operations for speed
# Each operation runs in background, we wait for all to complete

# 1. Kill tmux session (background)
(tmux kill-session -t devdock 2>/dev/null || true) &
pid_tmux=$!

# 2. Kill tilt processes (background)
(pkill -f 'tilt up' 2>/dev/null || true) &
pid_tilt=$!

# 3. Clean up Docker containers - optimized bulk operation (background)
(
  # Use -q for just IDs, then bulk remove (faster than xargs with names)
  containers=$(docker ps -a --filter 'name=devdock-' -q 2>/dev/null)
  if [ -n "$containers" ]; then
    docker rm -f $containers 2>/dev/null || true
  fi
) &
pid_docker=$!

# 4. Clean up DevDock database and pid files (background)
(rm -f ~/.devdock/devdock.db ~/.devdock/*.pid 2>/dev/null || true) &
pid_files=$!

# Wait for all parallel operations to complete
wait $pid_tmux $pid_tilt $pid_docker $pid_files

# 5. Run comprehensive devdock cleanup (sequential, after others complete)
# This ensures all resources are cleaned before running the full cleanup
cd /home/workspace/scio && devdock cleanup 2>/dev/null || true

echo "Cleanup complete"
"#;

    match ssh::ssh_exec_with_log(workspace, cleanup_script, log_file) {
        Ok(_) => {
            info!("Cleanup completed successfully");
            Ok(())
        }
        Err(e) => {
            // Log error but don't fail - cleanup is best-effort
            info!("Cleanup encountered errors (non-fatal): {}", e);
            Ok(())
        }
    }
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

/// Start DevDock on remote workspace with optional port forwarding.
#[allow(clippy::too_many_arguments)]
pub async fn start_remote_devdock<F>(
    state: &StateManager,
    workspace: &str,
    workspace_id: &str,
    profile: &str,
    profile_path: &Path,
    devdock_id: &str,
    services: Option<&str>,
    auto_forward: bool,
    offset: u16,
    copy_git_creds: bool,
    log_file: Option<&Path>,
    verbose: bool,
    on_config_complete: Option<F>,
) -> Result<Option<PortForwardManager>>
where
    F: FnOnce(),
{
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

    // 1. Sync configuration files
    info!("Syncing configuration to remote...");
    if verbose {
        println!("  Syncing configuration files...");
    }

    let custom_ini_path = find_custom_ini_path();
    let sync_status = config_sync::sync_all_config(
        workspace,
        profile_path,
        custom_ini_path.as_deref(),
        copy_git_creds,
        log_file,
        verbose,
    )?;
    if verbose {
        println!("  ✓ Configuration synced");
    }

    // Signal that config phase is complete (for progress UI)
    if let Some(callback) = on_config_complete {
        callback();
    }

    // Handle ADC expired (detected during config sync Phase 3, no extra SSH needed)
    if sync_status.adc_expired {
        println!();
        println!("╭─────────────────────────────────────────────────────╮");
        println!("│  ❌ ERROR: ADC Credentials EXPIRED                  │");
        println!("╰─────────────────────────────────────────────────────╯");
        println!();
        println!("DevDock requires valid Application Default Credentials (ADC).");
        println!("Your ADC credentials have expired and need to be refreshed.");
        println!();
        println!("To fix:");
        println!("  1. SSH to workspace: coder ssh {workspace}");
        println!("  2. Run: gcloud auth application-default login");
        println!("  3. Restart DevDock");
        println!();

        // Ask user if they want to continue anyway (for debugging)
        print!("Continue anyway? (DevDock will likely fail) [y/N] ");
        use std::io::{self, Write};
        io::stdout().flush().ok();

        let mut response = String::new();
        io::stdin().read_line(&mut response).ok();

        if !response.trim().eq_ignore_ascii_case("y") {
            return Err(RemoteError::SshError(
                "ADC credentials expired - please refresh and try again".to_string(),
            ));
        }

        println!("⚠️  Continuing with expired ADC (expect failures)...");
        println!();
    }

    // 2. Start DevDock in tmux session on remote
    // Export environment variables before tmux, they will be inherited by the session

    // Build environment variable exports (quote values to handle special characters)
    let mut env_exports =
        format!("export DEVDOCK_ID='{devdock_id}' && export DEVDOCK_PORT_OFFSET={offset}");

    // Export ALL AWS environment variables if set locally
    // This allows users to use either:
    //   1. Environment variables (export AWS_ACCESS_KEY_ID=...) OR
    //   2. Credential files (~/.aws/credentials + ~/.aws/config)
    // Both approaches are now fully supported and consistent
    let aws_env_vars = vec![
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AWS_REGION",
        "AWS_DEFAULT_REGION",
        "AWS_PROFILE",
    ];

    let mut exported_aws_vars = Vec::new();
    for var in aws_env_vars {
        if let Ok(value) = std::env::var(var) {
            // Quote the value to handle spaces and special characters
            env_exports.push_str(&format!(" && export {var}='{value}'"));
            exported_aws_vars.push(var);
        }
    }

    if verbose && !exported_aws_vars.is_empty() {
        println!("  AWS env vars exported: {}", exported_aws_vars.join(", "));
    }

    let log_path = diag::remote_start_log_path(devdock_id);
    let exit_path = diag::remote_start_exit_path(devdock_id);
    let meta_path = diag::remote_start_meta_path(devdock_id);
    let start_script = diag::build_remote_start_script(
        &env_exports,
        profile,
        services,
        &log_path,
        &exit_path,
        &meta_path,
    );

    info!("Starting DevDock in tmux session");
    if verbose {
        println!("  Starting DevDock on remote workspace...");
        println!("  Profile: {profile}");
        println!("  DevDock ID: {}", &devdock_id[..8.min(devdock_id.len())]);
        println!("  Port Offset: {offset}");
    }

    let remote_log_created = match ssh::ssh_exec_with_log(workspace, &start_script, log_file) {
        Ok(output) => {
            let created = diag::start_log_created(&output);
            if verbose {
                println!("  ✓ DevDock start command executed");
                if created {
                    println!("  Remote start log: coder ssh {workspace} -- tail -f {log_path}");
                }
            }
            // Don't print environment variable dumps
            if verbose
                && !output.trim().is_empty()
                && !output.trim().contains("tmux")
                && !output.trim().contains("declare -x")
                && !output.contains("DEVDOCK_LOG_CREATED")
            {
                println!("  Output: {}", output.trim());
            }
            created
        }
        Err(e) => {
            let err_msg = format!("{e}");
            // Check for duplicate session error
            if err_msg.contains("duplicate session") {
                println!("  ⚠ Duplicate tmux session detected, cleaning up...");

                // Kill existing session
                let _ = ssh::ssh_exec_with_log(
                    workspace,
                    "tmux kill-session -t devdock 2>/dev/null || true",
                    log_file,
                );
                sleep(Duration::from_secs(2));

                // Retry
                println!("  Retrying DevDock start...");
                match ssh::ssh_exec_with_log(workspace, &start_script, log_file) {
                    Ok(output) => {
                        println!("  ✓ Retry succeeded");
                        diag::start_log_created(&output)
                    }
                    Err(e2) => {
                        eprintln!("  ❌ ERROR: Retry failed: {e2}");
                        return Err(e2);
                    }
                }
            } else {
                eprintln!("  ❌ ERROR: DevDock start command failed: {e}");
                return Err(e);
            }
        }
    };

    if verbose {
        println!("  ✓ DevDock command started in background");
        if remote_log_created {
            println!("  To view logs: coder ssh {workspace} -- tail -f {log_path}");
        }
        println!("  Tmux: coder ssh {workspace} -- tmux attach -t devdock");
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
    // Always exits 0 so ssh_exec_with_log returns Ok; status is in stdout markers.
    // On tmux_dead, includes inner exit status and a bounded tail of the start log.
    let combined_check = diag::tilt_wait_check_script(&log_path, &exit_path);

    loop {
        let elapsed = start.elapsed().as_secs();

        match ssh::ssh_exec_with_log_timeout(
            workspace,
            &combined_check,
            log_file,
            Some(diag::REMOTE_TILT_WAIT_SSH_TIMEOUT_SECS),
        ) {
            Ok(output) if output.contains("DEVDOCK_STATUS:tilt_ready") => {
                if verbose {
                    println!("  ✓ Tilt process is running on port 10350!");
                }
                break;
            }
            Ok(output) if output.contains("DEVDOCK_STATUS:tmux_dead") => {
                let diag_info = diag::parse_tmux_dead_diagnostics(&output);
                eprintln!(
                    "\n  ❌ ERROR: {}",
                    diag::format_tmux_dead_message(workspace, profile, &diag_info)
                );
                return Err(RemoteError::CommandFailed(diag::command_failed_for_tmux_dead(
                    &diag_info,
                )));
            }
            Ok(_) => {
                // DEVDOCK_STATUS:waiting or unknown — not ready yet, continue
            }
            Err(RemoteError::SshTimeout(_)) => {
                // Agent slow/unreachable this iteration — keep waiting until the outer timeout
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
            eprintln!(
                "\n  ⚠ Warning: Tilt not detected on port 10350 after {} seconds",
                timeout.as_secs()
            );
            eprintln!("  DevDock may still be starting up. Check logs:");
            if remote_log_created {
                eprintln!("    coder ssh {workspace} -- tail -n 80 {log_path}");
            }
            eprintln!("    coder ssh {workspace} -- tmux attach -t devdock");
            break;
        }

        sleep(Duration::from_secs(2));
    }

    if verbose {
        println!();
        println!("  To view DevDock logs:");
        if remote_log_created {
            println!("    coder ssh {workspace} -- tail -f {log_path}");
        }
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

    // 5. Final success message (only in verbose mode, caller shows better summary)
    if verbose {
        println!("\n  ✅ DevDock is running!");
        println!("  Tilt UI: http://localhost:{}", offset + 10350);
        if remote_log_created {
            println!("  Start log: coder ssh {workspace} -- tail -f {log_path}");
        }
        println!("  Tmux: coder ssh {workspace} && tmux attach -t devdock");
    }

    Ok(port_mgr)
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
            "DEVDOCK_GIT_BRANCH\tfeature/foo\nDEVDOCK_GIT_STATUS_BEGIN\nDEVDOCK_GIT_STATUS_END\n",
        )
        .expect("parse clean state");

        assert_eq!(state.branch, "feature/foo");
        assert!(state.dirty_entries.is_empty());
        assert!(!state.is_dirty());
    }

    #[test]
    fn parse_remote_git_state_dirty() {
        let state = parse_remote_git_state(
            "DEVDOCK_GIT_BRANCH\tfeature/foo\nDEVDOCK_GIT_STATUS_BEGIN\n M java/Foo.java\n?? python_scio/new_file.py\nR  old.java -> new.java\nDEVDOCK_GIT_STATUS_END\n",
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
        let err = parse_remote_git_state("DEVDOCK_GIT_STATUS_BEGIN\nDEVDOCK_GIT_STATUS_END\n")
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
                Ok("DEVDOCK_GIT_BRANCH\tmain\nDEVDOCK_GIT_STATUS_BEGIN\nDEVDOCK_GIT_STATUS_END\n"
                    .to_string())
            })
            .expect("inspect with injected executor");

        assert_eq!(state.branch, "main");
        assert!(state.dirty_entries.is_empty());
    }
}
