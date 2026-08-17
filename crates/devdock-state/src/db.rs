//! Database management for DevDock state.

use crate::instance::{Instance, InstanceStatus};
use crate::session::{
    canonical_worktree, local_session_key, provisional_session_key, stable_session_key,
    ClaimedRemoteSession, RemoteSessionRequest, SessionManifestV1,
};
use crate::StatePaths;
use anyhow::{bail, Context, Result};
use sqlx::{sqlite::SqlitePool, Row, SqliteConnection};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// Kept in lockstep with migration::INSTANCE_COLUMNS_V1. Migration initialization runs before
// reconciliation or any query that uses this projection.
const INSTANCE_COLUMNS: &str = "id, workspace_path, workspace_name, workspace_id, \
    planned_workspace_name, session_key, branch, profile, mode, status, port_base, \
    failure_stage, failure_reason, is_quarantined, quarantine_reason, superseded_by, \
    manifest_path, last_observed_at, created_at, updated_at";

/// Manages DevDock state in SQLite database.
#[derive(Clone)]
pub struct StateManager {
    pool: SqlitePool,
    db_path: PathBuf,
    paths: StatePaths,
}

impl StateManager {
    /// Create a new state manager with the default database path.
    pub async fn new() -> Result<Self> {
        let home = std::env::var("HOME")?;
        let paths = StatePaths::from_home(Path::new(&home));
        paths.ensure_root()?;
        let db_path = paths.database();

        Self::with_path(&db_path).await
    }

    /// Create a state manager with a custom database path.
    pub async fn with_path(db_path: &Path) -> Result<Self> {
        let paths = StatePaths::from_database(db_path)?;
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let pool = SqlitePool::connect(&url).await?;

        let manager = Self { pool, db_path: db_path.to_path_buf(), paths };
        manager.initialize().await?;

        Ok(manager)
    }

    /// Initialize database schema.
    async fn initialize(&self) -> Result<()> {
        let mut connection = self.pool.acquire().await?;
        crate::migration::initialize(&mut connection, &self.db_path).await?;
        reconcile_persisted_sessions(&mut connection).await
    }

    /// Register a new DevDock instance.
    pub async fn register_instance(&self, instance: &Instance) -> Result<()> {
        let canonical = canonical_worktree(Path::new(&instance.workspace_path)).ok();
        let stored_workspace_path = canonical.as_deref().unwrap_or(&instance.workspace_path);
        let derived_session_key = if instance.session_key.is_none() {
            canonical.as_deref().and_then(|canonical| match instance.mode.as_str() {
                "local" => local_session_key(canonical).ok(),
                "remote" => instance
                    .workspace_id
                    .as_deref()
                    .and_then(|id| stable_session_key(canonical, id).ok())
                    .or_else(|| {
                        instance
                            .planned_workspace_name
                            .as_deref()
                            .or(instance.workspace_name.as_deref())
                            .and_then(|name| provisional_session_key(canonical, name).ok())
                    }),
                _ => None,
            })
        } else {
            None
        };
        let session_key = instance.session_key.as_ref().or(derived_session_key.as_ref());
        let planned_workspace_name = instance.planned_workspace_name.as_ref().or_else(|| {
            (instance.mode == "remote").then_some(instance.workspace_name.as_ref()).flatten()
        });

        sqlx::query(
            r"
            INSERT INTO instances (
                id, workspace_path, workspace_name, workspace_id, planned_workspace_name,
                session_key, branch, profile, mode, status, port_base, failure_stage,
                failure_reason, is_quarantined, quarantine_reason, superseded_by,
                manifest_path, last_observed_at, created_at, updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ",
        )
        .bind(&instance.id)
        .bind(stored_workspace_path)
        .bind(&instance.workspace_name)
        .bind(&instance.workspace_id)
        .bind(planned_workspace_name)
        .bind(session_key)
        .bind(&instance.branch)
        .bind(&instance.profile)
        .bind(&instance.mode)
        .bind(instance.status.as_str())
        .bind(i64::from(instance.port_base))
        .bind(&instance.failure_stage)
        .bind(&instance.failure_reason)
        .bind(if instance.is_quarantined { 1_i64 } else { 0_i64 })
        .bind(&instance.quarantine_reason)
        .bind(&instance.superseded_by)
        .bind(&instance.manifest_path)
        .bind(instance.last_observed_at)
        .bind(instance.created_at)
        .bind(instance.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Claim the one active remote session for a worktree/workspace-name pair.
    ///
    /// The immediate transaction serializes cross-process retries without adding a uniqueness
    /// constraint that would break schema-0 writers. Any duplicate active rows left by an older
    /// writer are deterministically superseded before this method returns.
    pub async fn claim_remote_session(
        &self,
        request: RemoteSessionRequest,
    ) -> Result<ClaimedRemoteSession> {
        let canonical = canonical_worktree(&request.workspace_path)?;
        let planned = request.planned_workspace_name.trim();
        if planned.is_empty() {
            bail!("planned workspace name cannot be empty");
        }
        let provisional_key = provisional_session_key(&canonical, planned)?;

        let mut connection = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *connection).await?;
        let result = claim_remote_session_in_transaction(
            &mut connection,
            &request,
            &canonical,
            planned,
            &provisional_key,
        )
        .await;
        let claimed = match result {
            Ok(claimed) => {
                sqlx::query("COMMIT").execute(&mut *connection).await?;
                claimed
            }
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                return Err(error);
            }
        };
        drop(connection);

        if let Err(error) = self.persist_session_manifest(&claimed.instance.id).await {
            let _ = self
                .record_failure(
                    &claimed.instance.id,
                    InstanceStatus::Error,
                    "session_manifest",
                    &error.to_string(),
                )
                .await;
            return Err(error);
        }

        let instance = self
            .get_instance(&claimed.instance.id)
            .await?
            .context("claimed remote session disappeared")?;
        Ok(ClaimedRemoteSession { instance, reused: claimed.reused })
    }

    /// Replace a provisional workspace-name identity with Coder's immutable workspace ID.
    pub async fn attach_remote_workspace(
        &self,
        instance_id: &str,
        workspace_id: &str,
        workspace_name: &str,
    ) -> Result<Instance> {
        let workspace_id = workspace_id.trim();
        let workspace_name = workspace_name.trim();
        if workspace_id.is_empty() || workspace_name.is_empty() {
            bail!("Coder workspace identity cannot be empty");
        }

        let mut connection = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *connection).await?;
        let result = attach_remote_workspace_in_transaction(
            &mut connection,
            instance_id,
            workspace_id,
            workspace_name,
        )
        .await;
        match result {
            Ok(()) => sqlx::query("COMMIT").execute(&mut *connection).await?,
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                return Err(error);
            }
        };
        drop(connection);

        self.persist_session_manifest(instance_id).await?;
        self.get_instance(instance_id).await?.context("attached remote session disappeared")
    }

    /// Persist lifecycle failure context without losing the retryable session record.
    pub async fn record_failure(
        &self,
        id: &str,
        status: InstanceStatus,
        stage: &str,
        reason: &str,
    ) -> Result<()> {
        let now = unix_timestamp()?;
        sqlx::query(
            "UPDATE instances
             SET status = ?, failure_stage = ?, failure_reason = ?, updated_at = ?,
                 last_observed_at = ?
             WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(stage)
        .bind(reason)
        .bind(now)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub fn paths(&self) -> &StatePaths {
        &self.paths
    }

    async fn persist_session_manifest(&self, id: &str) -> Result<()> {
        let instance = self.get_instance(id).await?.context("instance not found")?;
        let manifest = SessionManifestV1::from_instance(&instance)?;
        let manifest_path = manifest.write_atomic(&self.paths)?;
        let manifest_path =
            manifest_path.to_str().context("session manifest path is not valid UTF-8")?;
        let now = unix_timestamp()?;
        sqlx::query("UPDATE instances SET manifest_path = ?, updated_at = ? WHERE id = ?")
            .bind(manifest_path)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Get an instance by ID.
    pub async fn get_instance(&self, id: &str) -> Result<Option<Instance>> {
        let row = sqlx::query(&format!("SELECT {INSTANCE_COLUMNS} FROM instances WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(instance_from_row))
    }

    /// Get instance by workspace path.
    pub async fn get_by_workspace(&self, workspace_path: &str) -> Result<Option<Instance>> {
        let canonical = canonical_worktree(Path::new(workspace_path))
            .unwrap_or_else(|_| workspace_path.to_string());
        // Prefer Running > Starting > Stopping > Stopped > Error
        // Order by status priority, then most recently updated
        let row = sqlx::query(&format!(
            "SELECT {INSTANCE_COLUMNS}
             FROM instances
             WHERE workspace_path = ? OR workspace_path = ?
             ORDER BY
                CASE status
                    WHEN 'running' THEN 1
                    WHEN 'starting' THEN 2
                    WHEN 'stopping' THEN 3
                    WHEN 'stopped' THEN 4
                    ELSE 5
                END,
                updated_at DESC
             LIMIT 1"
        ))
        .bind(workspace_path)
        .bind(canonical)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(instance_from_row))
    }

    /// Alias for get_by_workspace for compatibility
    pub async fn get_instance_by_workspace(
        &self,
        workspace_path: &str,
    ) -> Result<Option<Instance>> {
        self.get_by_workspace(workspace_path).await
    }

    /// List all instances.
    pub async fn list_instances(&self) -> Result<Vec<Instance>> {
        let rows = sqlx::query(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM instances ORDER BY created_at DESC"
        ))
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(instance_from_row).collect())
    }

    /// Update instance status.
    pub async fn update_status(&self, id: &str, status: InstanceStatus) -> Result<()> {
        let now = unix_timestamp()?;

        sqlx::query(
            "UPDATE instances SET status = ?, failure_stage = NULL, failure_reason = NULL,
             updated_at = ?, last_observed_at = ? WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(now)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Update instance profile.
    pub async fn update_profile(&self, id: &str, profile: &str) -> Result<()> {
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;

        sqlx::query("UPDATE instances SET profile = ?, updated_at = ? WHERE id = ?")
            .bind(profile)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Delete an instance.
    pub async fn delete_instance(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM instances WHERE id = ?").bind(id).execute(&self.pool).await?;

        Ok(())
    }

    /// Save port mappings for a DevDock instance.
    pub async fn save_port_mappings(
        &self,
        devdock_id: &str,
        mappings: &[(String, String, u16, u16, Option<u32>)], // (service_name, port_type, local_port, remote_port, pid)
    ) -> Result<()> {
        // Delete existing mappings
        sqlx::query("DELETE FROM port_mappings WHERE devdock_id = ?")
            .bind(devdock_id)
            .execute(&self.pool)
            .await?;

        // Insert new mappings
        for (service_name, port_type, local_port, remote_port, pid) in mappings {
            sqlx::query(
                r"
                INSERT INTO port_mappings (devdock_id, service_name, port_type, local_port, remote_port, process_pid)
                VALUES (?, ?, ?, ?, ?, ?)
                ",
            )
            .bind(devdock_id)
            .bind(service_name)
            .bind(port_type)
            .bind(i64::from(*local_port))
            .bind(i64::from(*remote_port))
            .bind(pid.map(i64::from))
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    /// Get port mappings for a DevDock instance.
    pub async fn get_port_mappings(
        &self,
        devdock_id: &str,
    ) -> Result<Vec<(String, String, u16, u16, Option<u32>)>> {
        let rows = sqlx::query(
            "SELECT service_name, port_type, local_port, remote_port, process_pid FROM port_mappings WHERE devdock_id = ?"
        )
        .bind(devdock_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get("service_name"),
                    r.get("port_type"),
                    r.get::<i64, _>("local_port") as u16,
                    r.get::<i64, _>("remote_port") as u16,
                    r.get::<Option<i64>, _>("process_pid").map(|p| p as u32),
                )
            })
            .collect())
    }

    /// Save a single port mapping.
    pub async fn save_port_mapping(
        &self,
        devdock_id: &str,
        service_name: &str,
        port_type: &str,
        local_port: u16,
        remote_port: u16,
        pid: Option<u32>,
    ) -> Result<()> {
        // Delete existing mapping for this service
        sqlx::query("DELETE FROM port_mappings WHERE devdock_id = ? AND service_name = ?")
            .bind(devdock_id)
            .bind(service_name)
            .execute(&self.pool)
            .await?;

        // Insert new mapping
        sqlx::query(
            r"
            INSERT INTO port_mappings (devdock_id, service_name, port_type, local_port, remote_port, process_pid)
            VALUES (?, ?, ?, ?, ?, ?)
            ",
        )
        .bind(devdock_id)
        .bind(service_name)
        .bind(port_type)
        .bind(i64::from(local_port))
        .bind(i64::from(remote_port))
        .bind(pid.map(i64::from))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Remove a specific port mapping.
    pub async fn remove_port_mapping(&self, devdock_id: &str, service_name: &str) -> Result<()> {
        sqlx::query("DELETE FROM port_mappings WHERE devdock_id = ? AND service_name = ?")
            .bind(devdock_id)
            .bind(service_name)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Delete port mappings for a DevDock instance.
    pub async fn delete_port_mappings(&self, devdock_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM port_mappings WHERE devdock_id = ?")
            .bind(devdock_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Save sync session state.
    pub async fn save_sync_session(
        &self,
        devdock_id: &str,
        workspace_name: &str,
        pid: Option<u32>,
        full_mode: bool,
        status: &str,
    ) -> Result<()> {
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;

        sqlx::query(
            r"
            INSERT OR REPLACE INTO sync_sessions (devdock_id, workspace_name, pid, full_mode, last_sync_time, status)
            VALUES (?, ?, ?, ?, ?, ?)
            ",
        )
        .bind(devdock_id)
        .bind(workspace_name)
        .bind(pid.map(i64::from))
        .bind(i64::from(full_mode))
        .bind(now)
        .bind(status)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Update sync session liveness fields without rewriting sync metadata.
    pub async fn update_sync_session_liveness(
        &self,
        devdock_id: &str,
        pid: Option<u32>,
        status: &str,
    ) -> Result<()> {
        sqlx::query("UPDATE sync_sessions SET pid = ?, status = ? WHERE devdock_id = ?")
            .bind(pid.map(i64::from))
            .bind(status)
            .bind(devdock_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Get sync session state.
    pub async fn get_sync_session(
        &self,
        devdock_id: &str,
    ) -> Result<Option<(String, Option<u32>, bool, String)>> {
        let row = sqlx::query(
            "SELECT workspace_name, pid, full_mode, status FROM sync_sessions WHERE devdock_id = ?",
        )
        .bind(devdock_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| {
            (
                r.get("workspace_name"),
                r.get::<Option<i64>, _>("pid").map(|p| p as u32),
                r.get::<i64, _>("full_mode") != 0,
                r.get("status"),
            )
        }))
    }

    /// Delete sync session.
    pub async fn delete_sync_session(&self, devdock_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM sync_sessions WHERE devdock_id = ?")
            .bind(devdock_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}

async fn claim_remote_session_in_transaction(
    connection: &mut SqliteConnection,
    request: &RemoteSessionRequest,
    canonical: &str,
    planned: &str,
    provisional_key: &str,
) -> Result<ClaimedRemoteSession> {
    let rows = sqlx::query(&format!(
        "SELECT {INSTANCE_COLUMNS} FROM instances
         WHERE mode = 'remote' AND is_quarantined = 0 AND superseded_by IS NULL
           AND (
             status IN ('starting', 'running', 'stopping')
             OR (status IN ('stopped', 'error') AND failure_stage IS NOT NULL)
           )"
    ))
    .fetch_all(&mut *connection)
    .await?;

    let mut candidates: Vec<Instance> = rows
        .into_iter()
        .map(instance_from_row)
        .filter(|instance| {
            let stored_path = canonical_worktree(Path::new(&instance.workspace_path))
                .unwrap_or_else(|_| instance.workspace_path.clone());
            let stored_name =
                instance.planned_workspace_name.as_deref().or(instance.workspace_name.as_deref());
            stored_path == canonical && stored_name == Some(planned)
        })
        .collect();

    candidates.sort_by(|left, right| {
        left.workspace_id
            .is_some()
            .cmp(&right.workspace_id.is_some())
            .then_with(|| left.updated_at.cmp(&right.updated_at))
            .then_with(|| left.id.cmp(&right.id))
    });

    if let Some(mut winner) = candidates.pop() {
        if matches!(winner.status, InstanceStatus::Starting | InstanceStatus::Stopping) {
            bail!(
                "remote session conflict: session {} is already {}; wait for it to finish or stop it before retrying",
                winner.id,
                winner.status.as_str()
            );
        }

        for duplicate in candidates {
            sqlx::query(
                "UPDATE instances
                 SET status = 'stopped', superseded_by = ?, failure_stage = 'reconciliation',
                     failure_reason = 'duplicate_active_session', updated_at = ?
                 WHERE id = ?",
            )
            .bind(&winner.id)
            .bind(request.created_at)
            .bind(&duplicate.id)
            .execute(&mut *connection)
            .await?;
        }

        let session_key = if let Some(workspace_id) = winner.workspace_id.as_deref() {
            stable_session_key(canonical, workspace_id)?
        } else {
            provisional_key.to_string()
        };
        sqlx::query(
            "UPDATE instances
             SET workspace_path = ?, planned_workspace_name = ?, session_key = ?, branch = ?,
                 profile = ?, status = 'starting', failure_stage = NULL, failure_reason = NULL,
                 superseded_by = NULL, updated_at = ?, last_observed_at = ?
             WHERE id = ?",
        )
        .bind(canonical)
        .bind(planned)
        .bind(&session_key)
        .bind(&request.branch)
        .bind(&request.profile)
        .bind(request.created_at)
        .bind(request.created_at)
        .bind(&winner.id)
        .execute(&mut *connection)
        .await?;

        winner.workspace_path = canonical.to_string();
        winner.planned_workspace_name = Some(planned.to_string());
        winner.session_key = Some(session_key);
        winner.branch.clone_from(&request.branch);
        winner.profile.clone_from(&request.profile);
        winner.status = InstanceStatus::Starting;
        winner.failure_stage = None;
        winner.failure_reason = None;
        winner.superseded_by = None;
        winner.updated_at = request.created_at;
        winner.last_observed_at = Some(request.created_at);
        return Ok(ClaimedRemoteSession { instance: winner, reused: true });
    }

    let port_base = allocate_port_in_transaction(connection, request.requested_port_base).await?;
    let instance = Instance {
        id: request.proposed_id.clone(),
        workspace_path: canonical.to_string(),
        workspace_name: Some(planned.to_string()),
        workspace_id: None,
        planned_workspace_name: Some(planned.to_string()),
        session_key: Some(provisional_key.to_string()),
        branch: request.branch.clone(),
        profile: request.profile.clone(),
        mode: "remote".to_string(),
        status: InstanceStatus::Starting,
        port_base,
        failure_stage: None,
        failure_reason: None,
        is_quarantined: false,
        quarantine_reason: None,
        superseded_by: None,
        manifest_path: None,
        last_observed_at: Some(request.created_at),
        created_at: request.created_at,
        updated_at: request.created_at,
    };
    insert_instance(connection, &instance).await?;
    Ok(ClaimedRemoteSession { instance, reused: false })
}

async fn reconcile_persisted_sessions(connection: &mut SqliteConnection) -> Result<()> {
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *connection).await?;
    let result = reconcile_persisted_sessions_in_transaction(connection).await;
    match result {
        Ok(()) => {
            sqlx::query("COMMIT").execute(&mut *connection).await?;
            Ok(())
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}

async fn reconcile_persisted_sessions_in_transaction(
    connection: &mut SqliteConnection,
) -> Result<()> {
    let rows = sqlx::query(&format!(
        "SELECT {INSTANCE_COLUMNS} FROM instances
         WHERE status IN ('starting', 'running', 'stopping') AND is_quarantined = 0"
    ))
    .fetch_all(&mut *connection)
    .await?;
    let instances: Vec<Instance> = rows.into_iter().map(instance_from_row).collect();
    let mut active_by_key: HashMap<String, Vec<&Instance>> = HashMap::new();

    for instance in &instances {
        let key = canonical_worktree(Path::new(&instance.workspace_path)).and_then(|canonical| {
            match instance.mode.as_str() {
                "local" => local_session_key(&canonical),
                "remote" => {
                    if let Some(workspace_id) = instance
                        .workspace_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        stable_session_key(&canonical, workspace_id)
                    } else if let Some(planned) = instance
                        .planned_workspace_name
                        .as_deref()
                        .or(instance.workspace_name.as_deref())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        provisional_session_key(&canonical, planned)
                    } else {
                        bail!("remote session has no workspace identity")
                    }
                }
                _ => bail!("unknown session mode"),
            }
        });

        let key = match key {
            Ok(key) => key,
            Err(_) => {
                sqlx::query(
                    "UPDATE instances
                     SET status = 'quarantined', is_quarantined = 1,
                         quarantine_reason = 'session_identity_unresolvable',
                         last_observed_at = COALESCE(last_observed_at, updated_at)
                     WHERE id = ?",
                )
                .bind(&instance.id)
                .execute(&mut *connection)
                .await?;
                continue;
            }
        };

        let planned = if instance.mode == "remote" {
            instance.planned_workspace_name.as_deref().or(instance.workspace_name.as_deref())
        } else {
            None
        };
        sqlx::query(
            "UPDATE instances
             SET session_key = ?, planned_workspace_name = COALESCE(planned_workspace_name, ?),
                 last_observed_at = COALESCE(last_observed_at, updated_at)
             WHERE id = ?",
        )
        .bind(&key)
        .bind(planned)
        .bind(&instance.id)
        .execute(&mut *connection)
        .await?;
        active_by_key.entry(key).or_default().push(instance);
    }

    for duplicates in active_by_key.values_mut().filter(|group| group.len() > 1) {
        duplicates.sort_by(|left, right| {
            left.workspace_id
                .is_some()
                .cmp(&right.workspace_id.is_some())
                .then_with(|| left.updated_at.cmp(&right.updated_at))
                .then_with(|| left.id.cmp(&right.id))
        });
        let winner = duplicates.last().expect("duplicate group is not empty").id.clone();
        for duplicate in duplicates.iter().filter(|instance| instance.id != winner) {
            sqlx::query(
                "UPDATE instances
                 SET status = 'stopped', superseded_by = ?, failure_stage = 'reconciliation',
                     failure_reason = 'duplicate_active_session' WHERE id = ?",
            )
            .bind(&winner)
            .bind(&duplicate.id)
            .execute(&mut *connection)
            .await?;
        }
    }
    Ok(())
}

async fn attach_remote_workspace_in_transaction(
    connection: &mut SqliteConnection,
    instance_id: &str,
    workspace_id: &str,
    workspace_name: &str,
) -> Result<()> {
    let row = sqlx::query(&format!("SELECT {INSTANCE_COLUMNS} FROM instances WHERE id = ?"))
        .bind(instance_id)
        .fetch_optional(&mut *connection)
        .await?
        .context("instance not found")?;
    let instance = instance_from_row(row);
    if instance.mode != "remote" || instance.is_quarantined {
        bail!("cannot attach a workspace identity to this session");
    }

    let stable_key = stable_session_key(&instance.workspace_path, workspace_id)?;
    let now = unix_timestamp()?;
    sqlx::query(
        "UPDATE instances
         SET status = 'stopped', superseded_by = ?, failure_stage = 'reconciliation',
             failure_reason = 'duplicate_active_session', updated_at = ?
         WHERE id != ? AND mode = 'remote' AND status IN ('starting', 'running', 'stopping')
           AND is_quarantined = 0 AND workspace_id = ?",
    )
    .bind(instance_id)
    .bind(now)
    .bind(instance_id)
    .bind(workspace_id)
    .execute(&mut *connection)
    .await?;

    sqlx::query(
        "UPDATE instances
         SET workspace_id = ?, workspace_name = ?, session_key = ?, updated_at = ?,
             last_observed_at = ? WHERE id = ?",
    )
    .bind(workspace_id)
    .bind(workspace_name)
    .bind(stable_key)
    .bind(now)
    .bind(now)
    .bind(instance_id)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

async fn allocate_port_in_transaction(
    connection: &mut SqliteConnection,
    requested: Option<u16>,
) -> Result<u16> {
    let rows = sqlx::query(
        "SELECT port_base FROM instances
         WHERE status IN ('starting', 'running', 'stopping') AND is_quarantined = 0",
    )
    .fetch_all(&mut *connection)
    .await?;
    let used: Vec<u16> =
        rows.into_iter().map(|row| row.get::<i64, _>("port_base") as u16).collect();

    if let Some(port) = requested {
        if used.contains(&port) {
            bail!("requested port base {port} is already owned by another active session");
        }
        return Ok(port);
    }

    (0..65000)
        .step_by(1000)
        .find(|offset| !used.contains(offset))
        .context("no available port offsets")
}

async fn insert_instance(connection: &mut SqliteConnection, instance: &Instance) -> Result<()> {
    sqlx::query(
        "INSERT INTO instances (
            id, workspace_path, workspace_name, workspace_id, planned_workspace_name,
            session_key, branch, profile, mode, status, port_base, failure_stage,
            failure_reason, is_quarantined, quarantine_reason, superseded_by,
            manifest_path, last_observed_at, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&instance.id)
    .bind(&instance.workspace_path)
    .bind(&instance.workspace_name)
    .bind(&instance.workspace_id)
    .bind(&instance.planned_workspace_name)
    .bind(&instance.session_key)
    .bind(&instance.branch)
    .bind(&instance.profile)
    .bind(&instance.mode)
    .bind(instance.status.as_str())
    .bind(i64::from(instance.port_base))
    .bind(&instance.failure_stage)
    .bind(&instance.failure_reason)
    .bind(if instance.is_quarantined { 1_i64 } else { 0_i64 })
    .bind(&instance.quarantine_reason)
    .bind(&instance.superseded_by)
    .bind(&instance.manifest_path)
    .bind(instance.last_observed_at)
    .bind(instance.created_at)
    .bind(instance.updated_at)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

fn instance_from_row(row: sqlx::sqlite::SqliteRow) -> Instance {
    Instance {
        id: row.get("id"),
        workspace_path: row.get("workspace_path"),
        workspace_name: row.get("workspace_name"),
        workspace_id: row.get("workspace_id"),
        planned_workspace_name: row.get("planned_workspace_name"),
        session_key: row.get("session_key"),
        branch: row.get("branch"),
        profile: row.get("profile"),
        mode: row.get("mode"),
        status: InstanceStatus::parse(row.get("status")),
        port_base: row.get::<i64, _>("port_base") as u16,
        failure_stage: row.get("failure_stage"),
        failure_reason: row.get("failure_reason"),
        is_quarantined: row.get::<i64, _>("is_quarantined") != 0,
        quarantine_reason: row.get("quarantine_reason"),
        superseded_by: row.get("superseded_by"),
        manifest_path: row.get("manifest_path"),
        last_observed_at: row.get("last_observed_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn unix_timestamp() -> Result<i64> {
    Ok(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SESSION_MANIFEST_SCHEMA;
    use std::fs;
    use tempfile::TempDir;

    async fn state_fixture() -> (TempDir, TempDir, StateManager) {
        let state_dir = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let manager = StateManager::with_path(&state_dir.path().join("devdock.db")).await.unwrap();
        (state_dir, worktree, manager)
    }

    fn request(worktree: &Path, id: &str, workspace: &str, timestamp: i64) -> RemoteSessionRequest {
        RemoteSessionRequest {
            proposed_id: id.to_string(),
            workspace_path: worktree.to_path_buf(),
            planned_workspace_name: workspace.to_string(),
            branch: "feature/session-reuse".to_string(),
            profile: "qe_smoke_test".to_string(),
            requested_port_base: None,
            created_at: timestamp,
        }
    }

    #[tokio::test]
    async fn repeated_remote_claim_reuses_id_port_and_manifest() {
        let (_state_dir, worktree, manager) = state_fixture().await;

        let first = manager
            .claim_remote_session(request(worktree.path(), "dev-first", "workspace", 10))
            .await
            .unwrap();
        manager.update_status("dev-first", InstanceStatus::Running).await.unwrap();
        let second = manager
            .claim_remote_session(request(worktree.path(), "dev-second", "workspace", 20))
            .await
            .unwrap();

        assert!(!first.reused);
        assert!(second.reused);
        assert_eq!(second.instance.id, "dev-first");
        assert_eq!(second.instance.port_base, first.instance.port_base);
        assert_eq!(manager.list_instances().await.unwrap().len(), 1);

        let manifest_path = second.instance.manifest_path.as_ref().unwrap();
        let manifest: SessionManifestV1 =
            serde_json::from_slice(&fs::read(manifest_path).unwrap()).unwrap();
        assert_eq!(manifest.schema, SESSION_MANIFEST_SCHEMA);
        assert_eq!(manifest.instance_id, "dev-first");
        assert_eq!(manifest.planned_workspace_name, "workspace");
        assert_eq!(manifest.workspace_id, None);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(manifest_path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(
                fs::metadata(Path::new(manifest_path).parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[tokio::test]
    async fn attach_workspace_promotes_stable_identity_and_updates_manifest() {
        let (_state_dir, worktree, manager) = state_fixture().await;
        let claimed = manager
            .claim_remote_session(request(worktree.path(), "dev-session", "planned", 10))
            .await
            .unwrap();
        let provisional_key = claimed.instance.session_key.clone().unwrap();

        let attached = manager
            .attach_remote_workspace("dev-session", "coder-uuid", "actual-name")
            .await
            .unwrap();
        assert_eq!(attached.workspace_id.as_deref(), Some("coder-uuid"));
        assert_eq!(attached.workspace_name.as_deref(), Some("actual-name"));
        assert_ne!(attached.session_key.as_deref(), Some(provisional_key.as_str()));

        let manifest: SessionManifestV1 =
            serde_json::from_slice(&fs::read(attached.manifest_path.as_ref().unwrap()).unwrap())
                .unwrap();
        assert_eq!(manifest.workspace_id.as_deref(), Some("coder-uuid"));
        assert_eq!(manifest.workspace_name.as_deref(), Some("actual-name"));
    }

    #[tokio::test]
    async fn command_boundary_reconciles_old_writer_duplicates() {
        let (_state_dir, worktree, manager) = state_fixture().await;
        for (id, updated_at) in [("old", 10), ("new", 20)] {
            manager
                .register_instance(&Instance {
                    id: id.to_string(),
                    workspace_path: worktree.path().display().to_string(),
                    workspace_name: Some("workspace".to_string()),
                    workspace_id: None,
                    planned_workspace_name: None,
                    session_key: None,
                    branch: "master".to_string(),
                    profile: "default".to_string(),
                    mode: "remote".to_string(),
                    status: InstanceStatus::Running,
                    port_base: if id == "old" { 0 } else { 1000 },
                    failure_stage: None,
                    failure_reason: None,
                    is_quarantined: false,
                    quarantine_reason: None,
                    superseded_by: None,
                    manifest_path: None,
                    last_observed_at: None,
                    created_at: updated_at,
                    updated_at,
                })
                .await
                .unwrap();
        }

        let claimed = manager
            .claim_remote_session(request(worktree.path(), "unused", "workspace", 30))
            .await
            .unwrap();
        assert_eq!(claimed.instance.id, "new");
        let old = manager.get_instance("old").await.unwrap().unwrap();
        assert_eq!(old.status, InstanceStatus::Stopped);
        assert_eq!(old.superseded_by.as_deref(), Some("new"));
        assert_eq!(old.failure_stage.as_deref(), Some("reconciliation"));
        assert_eq!(old.failure_reason.as_deref(), Some("duplicate_active_session"));
    }

    #[tokio::test]
    async fn opening_state_reconciles_duplicates_from_an_old_writer() {
        let (state_dir, worktree, manager) = state_fixture().await;
        for (id, updated_at) in [("old", 10), ("new", 20)] {
            manager
                .register_instance(&Instance {
                    id: id.to_string(),
                    workspace_path: worktree.path().display().to_string(),
                    workspace_name: Some("workspace".to_string()),
                    workspace_id: None,
                    planned_workspace_name: None,
                    session_key: None,
                    branch: "master".to_string(),
                    profile: "default".to_string(),
                    mode: "remote".to_string(),
                    status: InstanceStatus::Running,
                    port_base: 0,
                    failure_stage: None,
                    failure_reason: None,
                    is_quarantined: false,
                    quarantine_reason: None,
                    superseded_by: None,
                    manifest_path: None,
                    last_observed_at: None,
                    created_at: updated_at,
                    updated_at,
                })
                .await
                .unwrap();
        }

        let reopened = StateManager::with_path(&state_dir.path().join("devdock.db")).await.unwrap();
        assert_eq!(
            reopened.get_instance("old").await.unwrap().unwrap().status,
            InstanceStatus::Stopped
        );
        assert!(reopened.get_instance("new").await.unwrap().unwrap().session_key.is_some());
    }

    #[tokio::test]
    async fn concurrent_claims_allow_only_one_remote_setup() {
        let state_dir = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let db_path = state_dir.path().join("devdock.db");
        let first_manager = StateManager::with_path(&db_path).await.unwrap();
        let second_manager = StateManager::with_path(&db_path).await.unwrap();

        let (first, second) = tokio::join!(
            first_manager.claim_remote_session(request(
                worktree.path(),
                "dev-concurrent-one",
                "workspace",
                10,
            )),
            second_manager.claim_remote_session(request(
                worktree.path(),
                "dev-concurrent-two",
                "workspace",
                11,
            )),
        );
        let outcomes = [first, second];
        assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
        let conflict = outcomes
            .iter()
            .find_map(|outcome| outcome.as_ref().err())
            .expect("one concurrent claim must be rejected");
        assert!(conflict.to_string().contains("remote session conflict"));
        assert_eq!(first_manager.list_instances().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn starting_or_stopping_session_rejects_reused_claim_without_mutation() {
        for status in [InstanceStatus::Starting, InstanceStatus::Stopping] {
            let (_state_dir, worktree, manager) = state_fixture().await;
            let first = manager
                .claim_remote_session(request(worktree.path(), "dev-owner", "workspace", 10))
                .await
                .unwrap();
            manager.update_status("dev-owner", status).await.unwrap();
            let before = manager.get_instance("dev-owner").await.unwrap().unwrap();

            let error = manager
                .claim_remote_session(request(worktree.path(), "dev-contender", "workspace", 20))
                .await
                .unwrap_err();
            assert!(error.to_string().contains("remote session conflict"));

            let after = manager.get_instance("dev-owner").await.unwrap().unwrap();
            assert_eq!(after.id, first.instance.id);
            assert_eq!(after.status, status);
            assert_eq!(after.updated_at, before.updated_at);
            assert_eq!(manager.list_instances().await.unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn distinct_planned_workspaces_get_distinct_sessions_and_ports() {
        let (_state_dir, worktree, manager) = state_fixture().await;
        let first = manager
            .claim_remote_session(request(worktree.path(), "dev-one", "one", 10))
            .await
            .unwrap();
        let second = manager
            .claim_remote_session(request(worktree.path(), "dev-two", "two", 20))
            .await
            .unwrap();

        assert_eq!(first.instance.id, "dev-one");
        assert_eq!(second.instance.id, "dev-two");
        assert_ne!(first.instance.port_base, second.instance.port_base);
    }

    #[tokio::test]
    async fn lifecycle_failure_is_persisted_and_cleared_on_retry() {
        let (_state_dir, worktree, manager) = state_fixture().await;
        manager
            .claim_remote_session(request(worktree.path(), "dev-session", "workspace", 10))
            .await
            .unwrap();
        manager
            .record_failure(
                "dev-session",
                InstanceStatus::Stopped,
                "sync_daemon",
                "transport failed",
            )
            .await
            .unwrap();
        let failed = manager.get_instance("dev-session").await.unwrap().unwrap();
        assert_eq!(failed.failure_stage.as_deref(), Some("sync_daemon"));
        assert_eq!(failed.failure_reason.as_deref(), Some("transport failed"));

        let retried = manager
            .claim_remote_session(request(worktree.path(), "unused", "workspace", 20))
            .await
            .unwrap();
        assert_eq!(retried.instance.id, "dev-session");
        assert!(retried.instance.failure_stage.is_none());
        assert!(retried.instance.failure_reason.is_none());
    }
}
