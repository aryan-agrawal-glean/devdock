//! Database management for DevDock state.

use crate::instance::{Instance, InstanceStatus};
use anyhow::Result;
use sqlx::{sqlite::SqlitePool, Row};
use std::path::{Path, PathBuf};

/// Manages DevDock state in SQLite database.
pub struct StateManager {
    pool: SqlitePool,
}

impl StateManager {
    /// Create a new state manager with the default database path.
    pub async fn new() -> Result<Self> {
        let home = std::env::var("HOME")?;
        let devdock_dir = PathBuf::from(home).join(".devdock");
        let db_path = devdock_dir.join("devdock.db");

        // Ensure directory exists
        std::fs::create_dir_all(&devdock_dir)?;

        Self::with_path(&db_path).await
    }

    /// Create a state manager with a custom database path.
    pub async fn with_path(db_path: &Path) -> Result<Self> {
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let pool = SqlitePool::connect(&url).await?;

        let manager = Self { pool };
        manager.initialize().await?;

        Ok(manager)
    }

    /// Initialize database schema.
    async fn initialize(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS instances (
                id TEXT PRIMARY KEY,
                workspace_path TEXT NOT NULL,
                branch TEXT NOT NULL,
                profile TEXT NOT NULL,
                mode TEXT NOT NULL,
                status TEXT NOT NULL,
                port_base INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS services (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                instance_id TEXT NOT NULL,
                name TEXT NOT NULL,
                status TEXT NOT NULL,
                port INTEGER,
                FOREIGN KEY (instance_id) REFERENCES instances(id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Port mappings table for remote DevDock
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS port_mappings (
                devdock_id TEXT NOT NULL,
                service_name TEXT NOT NULL,
                port_type TEXT NOT NULL,
                local_port INTEGER NOT NULL,
                remote_port INTEGER NOT NULL,
                process_pid INTEGER,
                PRIMARY KEY (devdock_id, service_name, port_type)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Sync sessions table for file sync
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sync_sessions (
                devdock_id TEXT PRIMARY KEY,
                workspace_name TEXT NOT NULL,
                pid INTEGER,
                full_mode BOOLEAN DEFAULT 0,
                last_sync_time INTEGER,
                files_synced INTEGER DEFAULT 0,
                status TEXT NOT NULL CHECK(status IN ('running', 'stopped', 'error'))
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Register a new DevDock instance.
    pub async fn register_instance(&self, instance: &Instance) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO instances (id, workspace_path, branch, profile, mode, status, port_base, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&instance.id)
        .bind(&instance.workspace_path)
        .bind(&instance.branch)
        .bind(&instance.profile)
        .bind(&instance.mode)
        .bind(instance.status.as_str())
        .bind(instance.port_base as i64)
        .bind(instance.created_at)
        .bind(instance.updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get an instance by ID.
    pub async fn get_instance(&self, id: &str) -> Result<Option<Instance>> {
        let row = sqlx::query(
            "SELECT id, workspace_path, branch, profile, mode, status, port_base, created_at, updated_at FROM instances WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| Instance {
            id: r.get("id"),
            workspace_path: r.get("workspace_path"),
            branch: r.get("branch"),
            profile: r.get("profile"),
            mode: r.get("mode"),
            status: InstanceStatus::parse(r.get("status")),
            port_base: r.get::<i64, _>("port_base") as u16,
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
        }))
    }

    /// Get instance by workspace path.
    pub async fn get_by_workspace(&self, workspace_path: &str) -> Result<Option<Instance>> {
        // Prefer Running > Starting > Stopping > Stopped > Error
        // Order by status priority, then most recently updated
        let row = sqlx::query(
            "SELECT id, workspace_path, branch, profile, mode, status, port_base, created_at, updated_at 
             FROM instances 
             WHERE workspace_path = ? 
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
        )
        .bind(workspace_path)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| Instance {
            id: r.get("id"),
            workspace_path: r.get("workspace_path"),
            branch: r.get("branch"),
            profile: r.get("profile"),
            mode: r.get("mode"),
            status: InstanceStatus::parse(r.get("status")),
            port_base: r.get::<i64, _>("port_base") as u16,
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
        }))
    }

    /// Alias for get_by_workspace for compatibility
    pub async fn get_instance_by_workspace(&self, workspace_path: &str) -> Result<Option<Instance>> {
        self.get_by_workspace(workspace_path).await
    }

    /// List all instances.
    pub async fn list_instances(&self) -> Result<Vec<Instance>> {
        let rows = sqlx::query(
            "SELECT id, workspace_path, branch, profile, mode, status, port_base, created_at, updated_at FROM instances ORDER BY created_at DESC"
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(|r| Instance {
            id: r.get("id"),
            workspace_path: r.get("workspace_path"),
            branch: r.get("branch"),
            profile: r.get("profile"),
            mode: r.get("mode"),
            status: InstanceStatus::parse(r.get("status")),
            port_base: r.get::<i64, _>("port_base") as u16,
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
        }).collect())
    }

    /// Update instance status.
    pub async fn update_status(&self, id: &str, status: InstanceStatus) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;

        sqlx::query(
            "UPDATE instances SET status = ?, updated_at = ? WHERE id = ?"
        )
        .bind(status.as_str())
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Delete an instance.
    pub async fn delete_instance(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM instances WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

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
                r#"
                INSERT INTO port_mappings (devdock_id, service_name, port_type, local_port, remote_port, process_pid)
                VALUES (?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(devdock_id)
            .bind(service_name)
            .bind(port_type)
            .bind(*local_port as i64)
            .bind(*remote_port as i64)
            .bind(pid.map(|p| p as i64))
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

        Ok(rows.into_iter().map(|r| (
            r.get("service_name"),
            r.get("port_type"),
            r.get::<i64, _>("local_port") as u16,
            r.get::<i64, _>("remote_port") as u16,
            r.get::<Option<i64>, _>("process_pid").map(|p| p as u32),
        )).collect())
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
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;

        sqlx::query(
            r#"
            INSERT OR REPLACE INTO sync_sessions (devdock_id, workspace_name, pid, full_mode, last_sync_time, status)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(devdock_id)
        .bind(workspace_name)
        .bind(pid.map(|p| p as i64))
        .bind(full_mode as i64)
        .bind(now)
        .bind(status)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get sync session state.
    pub async fn get_sync_session(&self, devdock_id: &str) -> Result<Option<(String, Option<u32>, bool, String)>> {
        let row = sqlx::query(
            "SELECT workspace_name, pid, full_mode, status FROM sync_sessions WHERE devdock_id = ?"
        )
        .bind(devdock_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| (
            r.get("workspace_name"),
            r.get::<Option<i64>, _>("pid").map(|p| p as u32),
            r.get::<i64, _>("full_mode") != 0,
            r.get("status"),
        )))
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

