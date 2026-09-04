//! Recovery and session-attachment commands for existing Coder workspaces.

use crate::commands::{ports, remote};
use crate::output;
use anyhow::{anyhow, Context, Result};
use devdock_remote::{check_coder_installed, coder, inspect_remote_runtime};
use devdock_state::{Instance, InstanceStatus, RemoteSessionRequest, SessionOrigin, StateManager};
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const SOURCE_PORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

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

    // Attach uses no-offset ports, matching the session-switcher contract.
    // The manager reuses only exact verified forwards and registers new ones
    // with the existing PID/start-time/command/workspace identity checks.
    ports::attach_workspace_for_recovery(&state, &target_info).await?;
    state.update_status(&instance.id, InstanceStatus::Running).await?;

    output::success(&format!(
        "Recovered remote session {} for workspace '{}'",
        Instance::display_id(&instance.id),
        target_identity.name
    ));
    if let Some(revision) = runtime.git_revision {
        println!("  Remote Git revision: {revision}");
    }
    println!(
        "  Retryable forwarding: devdock ports forward --workspace {} --all --no-offset",
        target_identity.name
    );
    Ok(())
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
