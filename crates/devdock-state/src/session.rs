//! Versioned remote-session identity and manifest handling.

use crate::forward::{PortMappingRecord, LOOPBACK_BIND_ADDRESS};
use crate::instance::Instance;
use crate::paths::{create_private_dir, StatePaths};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const SESSION_MANIFEST_SCHEMA: &str = "devdock.session/v1";

#[derive(Debug, Clone)]
pub struct RemoteSessionRequest {
    pub proposed_id: String,
    pub workspace_path: PathBuf,
    pub planned_workspace_name: String,
    pub branch: String,
    pub profile: String,
    pub requested_port_base: Option<u16>,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct ClaimedRemoteSession {
    pub instance: Instance,
    pub reused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionManifestV1 {
    pub schema: String,
    pub instance_id: String,
    pub canonical_worktree: String,
    pub planned_workspace_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_name: Option<String>,
    pub profile: String,
    pub branch: String,
    pub port_base: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub port_mappings: Vec<SessionPortMappingV1>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Non-secret, resolved forward data needed to operate an existing session
/// without re-reading its profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPortMappingV1 {
    pub service_name: String,
    pub port_type: String,
    pub bind_address: String,
    pub local_port: u16,
    pub remote_port: u16,
    pub process_pid: u32,
    pub workspace_id: String,
    pub owner_kind: String,
    pub process_start_time: u64,
    pub command_fingerprint: String,
}

impl SessionPortMappingV1 {
    pub fn from_record(record: &PortMappingRecord) -> Option<Self> {
        record.is_devdock_managed().then(|| Self {
            service_name: record.service_name.clone(),
            port_type: record.port_type.clone(),
            bind_address: LOOPBACK_BIND_ADDRESS.to_string(),
            local_port: record.local_port,
            remote_port: record.remote_port,
            process_pid: record.process_pid.expect("managed mapping has a PID"),
            workspace_id: record.workspace_id.clone().expect("managed mapping has a workspace ID"),
            owner_kind: record.owner_kind.clone().expect("managed mapping has an owner"),
            process_start_time: record
                .process_start_time
                .expect("managed mapping has a process start time"),
            command_fingerprint: record
                .command_fingerprint
                .clone()
                .expect("managed mapping has a command fingerprint"),
        })
    }
}

impl SessionManifestV1 {
    pub fn from_instance(instance: &Instance) -> Result<Self> {
        let planned_workspace_name = instance
            .planned_workspace_name
            .clone()
            .context("remote session is missing its planned workspace name")?;
        Ok(Self {
            schema: SESSION_MANIFEST_SCHEMA.to_string(),
            instance_id: instance.id.clone(),
            canonical_worktree: instance.workspace_path.clone(),
            planned_workspace_name,
            workspace_id: instance.workspace_id.clone(),
            workspace_name: instance.workspace_name.clone(),
            profile: instance.profile.clone(),
            branch: instance.branch.clone(),
            port_base: instance.port_base,
            port_mappings: Vec::new(),
            created_at: instance.created_at,
            updated_at: instance.updated_at,
        })
    }

    pub fn write_atomic(&self, paths: &StatePaths) -> Result<PathBuf> {
        paths.ensure_root()?;
        create_private_dir(&paths.instances())?;
        let instance_dir = paths.instance(&self.instance_id)?;
        create_private_dir(&instance_dir)?;
        let destination = paths.session_manifest(&self.instance_id)?;
        let temporary = instance_dir.join(format!(".session-{}.tmp", uuid::Uuid::new_v4()));
        let bytes = serde_json::to_vec_pretty(self)?;

        let write_result = (|| -> Result<()> {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&temporary)
                .with_context(|| format!("failed to create {}", temporary.display()))?;
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &destination).with_context(|| {
                format!("failed to atomically replace session manifest {}", destination.display())
            })?;
            sync_directory(&instance_dir)?;
            Ok(())
        })();

        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        write_result?;
        Ok(destination)
    }
}

pub(crate) fn canonical_worktree(path: &Path) -> Result<String> {
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("failed to canonicalize worktree {}", path.display()))?;
    canonical.to_str().map(str::to_string).context("canonical worktree path is not valid UTF-8")
}

pub(crate) fn provisional_session_key(
    canonical_worktree: &str,
    planned_workspace_name: &str,
) -> Result<String> {
    Ok(serde_json::to_string(&(
        1,
        canonical_worktree,
        "remote",
        "planned_workspace_name",
        planned_workspace_name,
    ))?)
}

pub(crate) fn local_session_key(canonical_worktree: &str) -> Result<String> {
    Ok(serde_json::to_string(&(1, canonical_worktree, "local", Option::<&str>::None))?)
}

pub(crate) fn stable_session_key(canonical_worktree: &str, workspace_id: &str) -> Result<String> {
    Ok(serde_json::to_string(&(1, canonical_worktree, "remote", "workspace_id", workspace_id))?)
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(path)?.sync_all()?;
    }
    Ok(())
}
