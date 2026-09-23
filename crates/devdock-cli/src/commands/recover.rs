//! Recovery and session-attachment commands for existing Coder workspaces.

use crate::commands::{ports, remote};
use crate::output;
use anyhow::{anyhow, Context, Result};
use devdock_config::ProfileLoader;
use devdock_remote::{check_coder_installed, coder, inspect_remote_runtime};
use devdock_state::{Instance, InstanceStatus, RemoteSessionRequest, SessionOrigin, StateManager};
use devdock_sync::{daemon as sync_daemon, universal_matcher};
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const SOURCE_PORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const RECOVERY_INITIAL_SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncRecoveryPlan {
    Reuse { pid: u32 },
    Restart { full_mode: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncRecoveryOutcome {
    Reused { pid: u32 },
    Restarted { pid: u32 },
}

/// Reconcile a manually started DevDock inside an existing Coder workspace.
///
/// Recovery is deliberately fail-closed: the local row is promoted to Running
/// only after remote tmux, Tilt, and the exact persisted DEVDOCK_ID match. It
/// never creates, stops, or deletes a Coder workspace.
pub async fn execute(
    workspace: &str,
    from_workspace: Option<&str>,
    profile: Option<&str>,
) -> Result<()> {
    check_coder_installed()?;
    let state = StateManager::new().await?;

    let target_identity = coder::get_workspace_identity(workspace)?
        .ok_or_else(|| anyhow!("Coder workspace '{workspace}' was not found"))?;

    refuse_quarantined_target(&state, &target_identity.id, &target_identity.name).await?;
    let target_is_known = state.list_instances().await?.into_iter().any(|instance| {
        instance.mode == "remote"
            && instance.superseded_by.is_none()
            && matches_target_instance(&instance, &target_identity.id, &target_identity.name)
    });
    if !target_is_known && profile.is_none() {
        return Err(anyhow!(
            "No local session is recorded for '{}'; provide the profile once: devdock recover --workspace {} --profile <profile-name>",
            target_identity.name,
            target_identity.name
        ));
    }

    if let Some(source) = from_workspace {
        let source_identity = coder::get_workspace_identity(source)?
            .ok_or_else(|| anyhow!("Coder workspace '{source}' was not found"))?;
        if source_identity.id == target_identity.id {
            return Err(anyhow!(
                "source and target workspace are the same; refusing a no-op handoff"
            ));
        }
    }

    let source_ports = if let Some(source) = from_workspace {
        let (source_id, ports) = ports::stop_workspace_for_attach(&state, source).await?;
        ports::wait_for_ports_free(&ports, SOURCE_PORT_TIMEOUT).await?;
        state.update_status(&source_id, InstanceStatus::Stopped).await?;
        Some(ports)
    } else {
        None
    };

    if let Some(ports) = &source_ports {
        output::success(&format!(
            "Stopped verified forwards and freed {} source port(s)",
            ports.len()
        ));
    }

    let runtime = inspect_remote_runtime(&target_identity.name, None).ok();
    let target_info = match ports::get_remote_instance(&state, Some(&target_identity.name)).await {
        Ok(info) => info,
        Err(error) if profile.is_some() && is_missing_local_session_error(&error) => {
            let runtime_id = runtime.as_ref().and_then(|snapshot| snapshot.devdock_id.clone());
            let claimed = state
                .claim_remote_session(RemoteSessionRequest {
                    proposed_id: runtime_id
                        .unwrap_or_else(|| Instance::generate_id(SessionOrigin::Dev)),
                    workspace_path: current_worktree()?,
                    planned_workspace_name: target_identity.name.clone(),
                    branch: current_branch()?,
                    profile: profile.expect("profile is present").to_string(),
                    requested_port_base: None,
                    created_at: now()?,
                })
                .await?;
            state
                .attach_remote_workspace(
                    &claimed.instance.id,
                    &target_identity.id,
                    &target_identity.name,
                )
                .await?;
            ports::get_remote_instance(&state, Some(&target_identity.name)).await?
        }
        Err(error) => {
            if let Some(snapshot) = &runtime {
                if snapshot.has_surviving_containers() {
                    remote::print_stale_container_warning(
                        snapshot,
                        &target_identity.name,
                        "unregistered",
                        None,
                    );
                }
            }
            return Err(error);
        }
    };

    let instance = state
        .get_instance(&target_info.devdock_id)
        .await?
        .context("remote session disappeared during recovery")?;
    if instance.is_quarantined || instance.status == InstanceStatus::Quarantined {
        return Err(anyhow!(
            "Refusing to recover quarantined remote session {}",
            Instance::display_id(&instance.id)
        ));
    }

    let runtime = runtime.ok_or_else(|| {
        anyhow!(
            "Could not inspect Coder workspace '{}'; session remains stopped and no forwards were attached",
            target_identity.name
        )
    })?;

    let git_branch_matches =
        runtime.git_branch.as_deref().map_or(true, |branch| branch == instance.branch);
    if !runtime.matches_session(&instance.id) || !git_branch_matches {
        if runtime.has_surviving_containers() {
            remote::print_stale_container_warning(
                &runtime,
                &target_identity.name,
                &instance.id,
                Some(&instance.branch),
            );
        }
        return Err(anyhow!(
            "remote runtime is not the requested session/revision: expected DEVDOCK_ID '{}', tmux, Tilt on port 10350, and Git branch '{}'; observed session {:?}, branch {:?}, revision {:?}",
            instance.id,
            instance.branch,
            runtime.devdock_id,
            runtime.git_branch,
            runtime.git_revision,
        ));
    }

    // Recover the exact session's sync daemon before creating local forwards.
    // This keeps a recovery failure from claiming an attached, Running session
    // when source propagation is no longer available.
    let sync_recovery = match recover_sync_daemon(&state, &instance, &target_identity.name).await {
        Ok(recovery) => recovery,
        Err(error) => {
            persist_recovery_failure(&state, &instance.id, "sync_daemon", &error).await;
            return Err(error);
        }
    };

    // Attach uses no-offset ports, matching the session-switcher contract.
    // The manager reuses only exact verified forwards and registers new ones
    // with the existing PID/start-time/command/workspace identity checks.
    if let Err(error) = ports::attach_workspace_for_recovery(&state, &target_info).await {
        persist_recovery_failure(&state, &instance.id, "port_forward", &error).await;
        return Err(error);
    }
    state.update_status(&instance.id, InstanceStatus::Running).await?;

    output::success(&format!(
        "Recovered remote session {} for workspace '{}'",
        Instance::display_id(&instance.id),
        target_identity.name
    ));
    if let Some(revision) = runtime.git_revision {
        println!("  Remote Git revision: {revision}");
    }
    match sync_recovery {
        SyncRecoveryOutcome::Reused { pid } => {
            println!("  Sync daemon: verified existing PID {pid}");
        }
        SyncRecoveryOutcome::Restarted { pid } => {
            println!("  Sync daemon: restarted and verified PID {pid}");
        }
    }
    println!(
        "  Retryable forwarding: devdock ports forward --workspace {} --all --no-offset",
        target_identity.name
    );
    Ok(())
}

async fn persist_recovery_failure(
    state: &StateManager,
    instance_id: &str,
    stage: &str,
    error: &anyhow::Error,
) {
    let reason = format!("{stage} recovery failed: {error}");
    if let Err(record_error) =
        state.record_failure(instance_id, InstanceStatus::Stopped, stage, &reason).await
    {
        tracing::warn!(%record_error, "Failed to persist recovery failure");
    }
}

/// Recover a dead or absent sync daemon using the persisted session settings.
/// A live daemon must have the exact instance command identity; two distinct
/// live PIDs are ambiguous and are left untouched rather than duplicated.
async fn recover_sync_daemon(
    state: &StateManager,
    instance: &Instance,
    workspace_name: &str,
) -> Result<SyncRecoveryOutcome> {
    let stored = state.get_sync_session(&instance.id).await?;
    let (stored_workspace, database_pid, full_mode) = stored
        .as_ref()
        .map(|(workspace, pid, full_mode, _)| (workspace.as_str(), *pid, *full_mode))
        .unwrap_or(("", None, false));
    if !stored_workspace.is_empty() && stored_workspace != workspace_name {
        return Err(anyhow!(
            "sync session for {} belongs to workspace '{}', not requested workspace '{}'",
            Instance::display_id(&instance.id),
            stored_workspace,
            workspace_name
        ));
    }

    let pid_file = sync_daemon::read_daemon_pid(&instance.id).ok().flatten();
    let database_pid_live =
        database_pid.filter(|pid| sync_daemon::is_daemon_running(&instance.id, *pid));
    let file_pid_live = pid_file.filter(|pid| sync_daemon::is_daemon_running(&instance.id, *pid));
    let plan = plan_sync_daemon_recovery(full_mode, database_pid_live, file_pid_live)?;

    if let SyncRecoveryPlan::Reuse { pid } = plan {
        state.update_sync_session_liveness(&instance.id, Some(pid), "running").await?;
        return Ok(SyncRecoveryOutcome::Reused { pid });
    }

    // The database must not keep a dead PID marked running while the fresh
    // daemon is being established. Persist error first, then replace it only
    // after the exact child and initial watermark are verified.
    state.save_sync_session(&instance.id, workspace_name, None, full_mode, "error").await?;

    let force_patterns = ProfileLoader::new()
        .load(&instance.profile)
        .map_err(|error| {
            anyhow!("could not load recovery sync profile '{}': {error}", instance.profile)
        })?
        .sync
        .force_patterns;
    let repo_path = PathBuf::from(&instance.workspace_path);
    let branch_mode = instance.branch != "master";
    let pid = match sync_daemon::start_daemon(
        &instance.id,
        workspace_name,
        &repo_path,
        full_mode,
        branch_mode,
        "info",
        &force_patterns,
        universal_matcher(),
    ) {
        Ok(pid) => pid,
        Err(error) => {
            state.save_sync_session(&instance.id, workspace_name, None, full_mode, "error").await?;
            return Err(error.into());
        }
    };

    if !sync_daemon::is_daemon_running(&instance.id, pid) {
        state.save_sync_session(&instance.id, workspace_name, None, full_mode, "error").await?;
        return Err(anyhow!("restarted sync daemon PID {pid} is not alive"));
    }
    if let Err(error) = sync_daemon::wait_for_initial_sync_watermark(
        &instance.id,
        workspace_name,
        &repo_path,
        pid,
        RECOVERY_INITIAL_SYNC_TIMEOUT,
    ) {
        state.save_sync_session(&instance.id, workspace_name, None, full_mode, "error").await?;
        return Err(error.into());
    }
    let recorded_pid = sync_daemon::read_daemon_pid(&instance.id)?;
    if recorded_pid != Some(pid) || !sync_daemon::is_daemon_running(&instance.id, pid) {
        state.save_sync_session(&instance.id, workspace_name, None, full_mode, "error").await?;
        return Err(anyhow!(
            "restarted sync daemon PID/state does not match its initial watermark"
        ));
    }

    state.save_sync_session(&instance.id, workspace_name, Some(pid), full_mode, "running").await?;
    Ok(SyncRecoveryOutcome::Restarted { pid })
}

fn plan_sync_daemon_recovery(
    full_mode: bool,
    database_pid_live: Option<u32>,
    file_pid_live: Option<u32>,
) -> Result<SyncRecoveryPlan> {
    match (database_pid_live, file_pid_live) {
        (Some(database_pid), Some(file_pid)) if database_pid != file_pid => Err(anyhow!(
            "two distinct live sync daemon PIDs ({database_pid} and {file_pid}) claim this session"
        )),
        (Some(pid), _) | (_, Some(pid)) => Ok(SyncRecoveryPlan::Reuse { pid }),
        (None, None) => Ok(SyncRecoveryPlan::Restart { full_mode }),
    }
}

fn is_missing_local_session_error(error: &anyhow::Error) -> bool {
    error.to_string().contains("No local remote session is recorded")
}

async fn refuse_quarantined_target(
    state: &StateManager,
    workspace_id: &str,
    workspace_name: &str,
) -> Result<()> {
    let instances = state.list_instances().await?;
    if let Some(instance) = instances.into_iter().find(|instance| {
        instance.mode == "remote"
            && (instance.workspace_id.as_deref() == Some(workspace_id)
                || instance.workspace_name.as_deref() == Some(workspace_name)
                || instance.planned_workspace_name.as_deref() == Some(workspace_name))
            && (instance.is_quarantined || instance.status == InstanceStatus::Quarantined)
    }) {
        return Err(anyhow!(
            "Refusing to mutate quarantined remote session {} for workspace '{}'",
            Instance::display_id(&instance.id),
            workspace_name
        ));
    }
    Ok(())
}

fn matches_target_instance(instance: &Instance, workspace_id: &str, workspace_name: &str) -> bool {
    match instance.workspace_id.as_deref() {
        Some(stored_id) => stored_id == workspace_id,
        None => {
            instance.workspace_name.as_deref() == Some(workspace_name)
                || instance.planned_workspace_name.as_deref() == Some(workspace_name)
        }
    }
}

fn current_worktree() -> Result<PathBuf> {
    let output = Command::new("git").args(["rev-parse", "--show-toplevel"]).output()?;
    if !output.status.success() {
        return Err(anyhow!("not in a Git worktree"));
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}

fn current_branch() -> Result<String> {
    let output = Command::new("git").args(["rev-parse", "--abbrev-ref", "HEAD"]).output()?;
    if !output.status.success() {
        return Err(anyhow!("could not determine current Git revision"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn now() -> Result<i64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_sync_daemon_restarts_then_a_healthy_daemon_is_reused() {
        let restarted = plan_sync_daemon_recovery(false, None, None).unwrap();
        assert_eq!(restarted, SyncRecoveryPlan::Restart { full_mode: false });

        let reused = plan_sync_daemon_recovery(false, Some(4242), Some(4242)).unwrap();
        assert_eq!(reused, SyncRecoveryPlan::Reuse { pid: 4242 });
    }

    #[test]
    fn conflicting_live_sync_daemons_are_not_adopted_or_duplicated() {
        let error = plan_sync_daemon_recovery(true, Some(4242), Some(4343)).unwrap_err();
        assert!(error.to_string().contains("two distinct live sync daemon PIDs"));
    }
}
