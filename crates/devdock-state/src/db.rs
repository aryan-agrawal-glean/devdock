//! Database management for DevDock state.

use crate::instance::{Instance, InstanceStatus};
use anyhow::Result;
use sqlx::{sqlite::SqlitePool, Row};
use std::path::PathBuf;

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
    pub async fn with_path(db_path: &PathBuf) -> Result<Self> {
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
            status: InstanceStatus::from_str(r.get("status")),
            port_base: r.get::<i64, _>("port_base") as u16,
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
        }))
    }
    
    /// Get instance by workspace path.
    pub async fn get_by_workspace(&self, workspace_path: &str) -> Result<Option<Instance>> {
        let row = sqlx::query(
            "SELECT id, workspace_path, branch, profile, mode, status, port_base, created_at, updated_at FROM instances WHERE workspace_path = ?"
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
            status: InstanceStatus::from_str(r.get("status")),
            port_base: r.get::<i64, _>("port_base") as u16,
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
        }))
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
            status: InstanceStatus::from_str(r.get("status")),
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
}

