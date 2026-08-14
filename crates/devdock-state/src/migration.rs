//! Versioned, transactional migrations for the legacy DevDock SQLite database.

use anyhow::{anyhow, bail, Context, Result};
use libsqlite3_sys as ffi;
use sqlx::{Connection, Row, SqliteConnection};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::ptr;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const CURRENT_SCHEMA_VERSION: i64 = 1;

const CREATE_SCHEMA_V1: &[&str] = &[
    r"CREATE TABLE IF NOT EXISTS instances (
        id TEXT PRIMARY KEY,
        workspace_path TEXT NOT NULL,
        workspace_name TEXT,
        branch TEXT NOT NULL,
        profile TEXT NOT NULL,
        mode TEXT NOT NULL,
        status TEXT NOT NULL,
        port_base INTEGER NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        workspace_id TEXT,
        planned_workspace_name TEXT,
        session_key TEXT,
        failure_stage TEXT,
        failure_reason TEXT,
        is_quarantined INTEGER NOT NULL DEFAULT 0,
        quarantine_reason TEXT,
        superseded_by TEXT,
        manifest_path TEXT,
        last_observed_at INTEGER
    )",
    r"CREATE TABLE IF NOT EXISTS services (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        instance_id TEXT NOT NULL,
        name TEXT NOT NULL,
        status TEXT NOT NULL,
        port INTEGER,
        FOREIGN KEY (instance_id) REFERENCES instances(id) ON DELETE CASCADE
    )",
    r"CREATE TABLE IF NOT EXISTS port_mappings (
        devdock_id TEXT NOT NULL,
        service_name TEXT NOT NULL,
        port_type TEXT NOT NULL,
        local_port INTEGER NOT NULL,
        remote_port INTEGER NOT NULL,
        process_pid INTEGER,
        workspace_id TEXT,
        owner_kind TEXT,
        process_start_time INTEGER,
        command_fingerprint TEXT,
        PRIMARY KEY (devdock_id, service_name, port_type)
    )",
    r"CREATE TABLE IF NOT EXISTS sync_sessions (
        devdock_id TEXT PRIMARY KEY,
        workspace_name TEXT NOT NULL,
        pid INTEGER,
        full_mode BOOLEAN DEFAULT 0,
        last_sync_time INTEGER,
        files_synced INTEGER DEFAULT 0,
        status TEXT NOT NULL CHECK(status IN ('running', 'stopped', 'error'))
    )",
];

const INSTANCE_COLUMNS_V1: &[(&str, &str)] = &[
    ("workspace_name", "TEXT"),
    ("workspace_id", "TEXT"),
    ("planned_workspace_name", "TEXT"),
    ("session_key", "TEXT"),
    ("failure_stage", "TEXT"),
    ("failure_reason", "TEXT"),
    ("is_quarantined", "INTEGER NOT NULL DEFAULT 0"),
    ("quarantine_reason", "TEXT"),
    ("superseded_by", "TEXT"),
    ("manifest_path", "TEXT"),
    ("last_observed_at", "INTEGER"),
];

const PORT_MAPPING_COLUMNS_V1: &[(&str, &str)] = &[
    ("workspace_id", "TEXT"),
    ("owner_kind", "TEXT"),
    ("process_start_time", "INTEGER"),
    ("command_fingerprint", "TEXT"),
];

pub(crate) async fn initialize(connection: &mut SqliteConnection, db_path: &Path) -> Result<()> {
    let version = schema_version(connection).await?;
    if version > CURRENT_SCHEMA_VERSION {
        bail!(
            "DevDock state database uses unsupported schema version {version}; this binary supports up to {CURRENT_SCHEMA_VERSION}. Update DevDock before modifying this database"
        );
    }

    if version == CURRENT_SCHEMA_VERSION {
        return Ok(());
    }

    if has_table(connection, "instances").await? {
        create_schema0_backup(connection, db_path).await?;
        migrate_0_to_1(connection).await
    } else {
        create_fresh_schema_v1(connection).await
    }
}

async fn schema_version(connection: &mut SqliteConnection) -> Result<i64> {
    Ok(sqlx::query_scalar("PRAGMA user_version").fetch_one(&mut *connection).await?)
}

async fn has_table(connection: &mut SqliteConnection, table: &str) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
    )
    .bind(table)
    .fetch_one(&mut *connection)
    .await?
        != 0)
}

async fn create_fresh_schema_v1(connection: &mut SqliteConnection) -> Result<()> {
    let mut transaction = connection.begin().await?;
    for statement in CREATE_SCHEMA_V1 {
        sqlx::query(statement).execute(&mut *transaction).await?;
    }
    sqlx::query(&format!("PRAGMA user_version = {CURRENT_SCHEMA_VERSION}"))
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

async fn migrate_0_to_1(connection: &mut SqliteConnection) -> Result<()> {
    let mut transaction = connection.begin().await?;

    for statement in CREATE_SCHEMA_V1 {
        sqlx::query(statement).execute(&mut *transaction).await?;
    }
    add_missing_columns(&mut transaction, "instances", INSTANCE_COLUMNS_V1).await?;
    add_missing_columns(&mut transaction, "sync_sessions", &[("pid", "INTEGER")]).await?;
    add_missing_columns(&mut transaction, "port_mappings", PORT_MAPPING_COLUMNS_V1).await?;
    backfill_legacy_sessions(&mut transaction).await?;

    sqlx::query(&format!("PRAGMA user_version = {CURRENT_SCHEMA_VERSION}"))
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

async fn add_missing_columns(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &str,
    expected: &[(&str, &str)],
) -> Result<()> {
    let existing = table_columns(transaction, table).await?;
    for (column, definition) in expected {
        if !existing.iter().any(|value| value == column) {
            let statement = format!("ALTER TABLE {table} ADD COLUMN {column} {definition}");
            sqlx::query(&statement).execute(&mut **transaction).await?;
        }
    }
    Ok(())
}

async fn table_columns(connection: &mut SqliteConnection, table: &str) -> Result<Vec<String>> {
    let rows =
        sqlx::query(&format!("PRAGMA table_info({table})")).fetch_all(&mut *connection).await?;
    Ok(rows.into_iter().map(|row| row.get("name")).collect())
}

#[derive(Debug)]
struct LegacyInstance {
    id: String,
    status: String,
    updated_at: i64,
    session_key: Option<String>,
}

async fn backfill_legacy_sessions(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT id, workspace_path, workspace_name, mode, status, updated_at FROM instances",
    )
    .fetch_all(&mut **transaction)
    .await?;

    let mut instances = Vec::with_capacity(rows.len());
    for row in rows {
        let id: String = row.get("id");
        let workspace_path: String = row.get("workspace_path");
        let workspace_name: Option<String> = row.get("workspace_name");
        let mode: String = row.get("mode");
        let status: String = row.get("status");
        let updated_at: i64 = row.get("updated_at");
        let session_key = legacy_session_key(&workspace_path, &mode, workspace_name.as_deref());
        let planned_workspace_name = if mode == "remote" {
            workspace_name.as_deref().map(str::trim).filter(|value| !value.is_empty())
        } else {
            None
        };

        if let Some(key) = &session_key {
            sqlx::query(
                "UPDATE instances SET session_key = ?, planned_workspace_name = ?, last_observed_at = updated_at WHERE id = ?",
            )
            .bind(key)
            .bind(planned_workspace_name)
            .bind(&id)
            .execute(&mut **transaction)
            .await?;
        } else if is_active_status(&status) {
            sqlx::query(
                "UPDATE instances SET status = 'quarantined', is_quarantined = 1, quarantine_reason = 'legacy_session_identity_ambiguous', last_observed_at = updated_at WHERE id = ?",
            )
            .bind(&id)
            .execute(&mut **transaction)
            .await?;
        }

        instances.push(LegacyInstance { id, status, updated_at, session_key });
    }

    let mut active_by_key: HashMap<String, Vec<&LegacyInstance>> = HashMap::new();
    for instance in &instances {
        if is_active_status(&instance.status) {
            if let Some(key) = &instance.session_key {
                active_by_key.entry(key.clone()).or_default().push(instance);
            }
        }
    }

    for duplicates in active_by_key.values_mut().filter(|values| values.len() > 1) {
        duplicates.sort_by(|left, right| {
            left.updated_at.cmp(&right.updated_at).then_with(|| left.id.cmp(&right.id))
        });
        let winner = duplicates.last().expect("duplicate group is not empty").id.clone();
        for duplicate in duplicates.iter().filter(|instance| instance.id != winner) {
            sqlx::query(
                "UPDATE instances SET status = 'stopped', superseded_by = ?, failure_stage = 'migration', failure_reason = 'duplicate_active_session' WHERE id = ?",
            )
            .bind(&winner)
            .bind(&duplicate.id)
            .execute(&mut **transaction)
            .await?;
        }
    }

    Ok(())
}

fn legacy_session_key(
    workspace_path: &str,
    mode: &str,
    workspace_name: Option<&str>,
) -> Option<String> {
    let canonical = fs::canonicalize(workspace_path).ok()?;
    let workspace_name = match mode {
        "local" => None,
        "remote" => Some(workspace_name.map(str::trim).filter(|value| !value.is_empty())?),
        _ => return None,
    };
    serde_json::to_string(&(1, canonical.to_str()?, mode, workspace_name)).ok()
}

fn is_active_status(status: &str) -> bool {
    matches!(status, "starting" | "running" | "stopping")
}

async fn create_schema0_backup(
    connection: &mut SqliteConnection,
    db_path: &Path,
) -> Result<PathBuf> {
    sqlx::query("PRAGMA wal_checkpoint(FULL)").execute(&mut *connection).await?;

    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let file_name = db_path
        .file_name()
        .and_then(|value| value.to_str())
        .context("state database path has no UTF-8 file name")?;
    let backup_path = db_path.with_file_name(format!("{file_name}.schema0.{timestamp}.bak"));
    if let Err(error) = online_backup(connection, &backup_path).await {
        let _ = fs::remove_file(&backup_path);
        return Err(error).context("failed to create restorable schema-0 backup");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&backup_path, fs::Permissions::from_mode(0o600))?;
    }
    File::open(&backup_path)?.sync_all()?;
    #[cfg(unix)]
    File::open(backup_path.parent().context("backup path has no parent")?)?.sync_all()?;
    Ok(backup_path)
}

async fn online_backup(connection: &mut SqliteConnection, backup_path: &Path) -> Result<()> {
    let backup_path = CString::new(backup_path.as_os_str().to_string_lossy().as_bytes())?;
    let main = b"main\0";
    let mut destination = ptr::null_mut();

    // SAFETY: `backup_path` and `main` are live C strings, `destination` is initialized by SQLite,
    // and every successfully opened handle is closed before returning.
    let open_result = unsafe {
        ffi::sqlite3_open_v2(
            backup_path.as_ptr(),
            &mut destination,
            ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE,
            ptr::null(),
        )
    };
    if open_result != ffi::SQLITE_OK {
        let message = sqlite_error(destination);
        if !destination.is_null() {
            // SAFETY: SQLite returned this handle from sqlite3_open_v2.
            unsafe { ffi::sqlite3_close(destination) };
        }
        bail!("failed to open schema-0 backup database: {message}");
    }

    let result = async {
        let mut source = connection.lock_handle().await?;
        // SAFETY: both database handles are valid and locked for the duration of the backup.
        let backup = unsafe {
            ffi::sqlite3_backup_init(
                destination,
                main.as_ptr().cast(),
                source.as_raw_handle().as_ptr(),
                main.as_ptr().cast(),
            )
        };
        if backup.is_null() {
            bail!("failed to initialize schema-0 backup: {}", sqlite_error(destination));
        }

        // SAFETY: `backup` is a live sqlite3_backup handle until sqlite3_backup_finish below.
        let step_result = unsafe { ffi::sqlite3_backup_step(backup, -1) };
        // SAFETY: every successful sqlite3_backup_init must be paired with finish exactly once.
        let finish_result = unsafe { ffi::sqlite3_backup_finish(backup) };
        if step_result != ffi::SQLITE_DONE || finish_result != ffi::SQLITE_OK {
            bail!("failed to copy schema-0 database: {}", sqlite_error(destination));
        }
        Ok(())
    }
    .await;

    // SAFETY: `destination` is a valid handle opened above and no backup handle remains live.
    let close_result = unsafe { ffi::sqlite3_close(destination) };
    result?;
    if close_result != ffi::SQLITE_OK {
        return Err(anyhow!("failed to close schema-0 backup database ({close_result})"));
    }
    Ok(())
}

fn sqlite_error(handle: *mut ffi::sqlite3) -> String {
    if handle.is_null() {
        return "unknown SQLite error".to_string();
    }
    // SAFETY: `handle` is live and sqlite3_errmsg returns a NUL-terminated string owned by it.
    unsafe { CStr::from_ptr(ffi::sqlite3_errmsg(handle)) }.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;
    use tempfile::TempDir;

    async fn connect(path: &Path) -> SqliteConnection {
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .unwrap()
            .create_if_missing(true);
        SqliteConnection::connect_with(&options).await.unwrap()
    }

    async fn create_schema0(
        path: &Path,
        has_workspace_name: bool,
        has_sync_pid: bool,
    ) -> SqliteConnection {
        let mut connection = connect(path).await;
        let workspace_name = if has_workspace_name { ", workspace_name TEXT" } else { "" };
        sqlx::query(&format!(
            "CREATE TABLE instances (
                id TEXT PRIMARY KEY,
                workspace_path TEXT NOT NULL
                {workspace_name},
                branch TEXT NOT NULL,
                profile TEXT NOT NULL,
                mode TEXT NOT NULL,
                status TEXT NOT NULL,
                port_base INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )"
        ))
        .execute(&mut connection)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE services (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                instance_id TEXT NOT NULL,
                name TEXT NOT NULL,
                status TEXT NOT NULL,
                port INTEGER
            )",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE port_mappings (
                devdock_id TEXT NOT NULL,
                service_name TEXT NOT NULL,
                port_type TEXT NOT NULL,
                local_port INTEGER NOT NULL,
                remote_port INTEGER NOT NULL,
                process_pid INTEGER,
                PRIMARY KEY (devdock_id, service_name, port_type)
            )",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        let pid = if has_sync_pid { ", pid INTEGER" } else { "" };
        sqlx::query(&format!(
            "CREATE TABLE sync_sessions (
                devdock_id TEXT PRIMARY KEY,
                workspace_name TEXT NOT NULL
                {pid},
                full_mode BOOLEAN DEFAULT 0,
                last_sync_time INTEGER,
                files_synced INTEGER DEFAULT 0,
                status TEXT NOT NULL CHECK(status IN ('running', 'stopped', 'error'))
            )"
        ))
        .execute(&mut connection)
        .await
        .unwrap();
        connection
    }

    fn database_path(directory: &TempDir) -> PathBuf {
        directory.path().join("devdock.db")
    }

    fn backups(directory: &TempDir) -> Vec<PathBuf> {
        let mut backups: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(".schema0.") && name.ends_with(".bak"))
                    .then_some(path)
            })
            .collect();
        backups.sort();
        backups
    }

    async fn insert_legacy_instance(
        connection: &mut SqliteConnection,
        id: &str,
        workspace_path: &Path,
        workspace_name: Option<&str>,
        mode: &str,
        status: &str,
        updated_at: i64,
    ) {
        sqlx::query(
            "INSERT INTO instances (
                id, workspace_path, workspace_name, branch, profile, mode, status,
                port_base, created_at, updated_at
             ) VALUES (?, ?, ?, 'main', 'test', ?, ?, 0, 1, ?)",
        )
        .bind(id)
        .bind(workspace_path.to_string_lossy().as_ref())
        .bind(workspace_name)
        .bind(mode)
        .bind(status)
        .bind(updated_at)
        .execute(connection)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn fresh_database_is_created_at_schema_v1_without_legacy_backup() {
        let directory = tempfile::tempdir().unwrap();
        let path = database_path(&directory);
        let mut connection = connect(&path).await;

        initialize(&mut connection, &path).await.unwrap();

        assert_eq!(schema_version(&mut connection).await.unwrap(), CURRENT_SCHEMA_VERSION);
        assert!(table_columns(&mut connection, "instances")
            .await
            .unwrap()
            .contains(&"session_key".to_string()));
        assert!(backups(&directory).is_empty());
    }

    #[tokio::test]
    async fn migrates_every_known_schema0_optional_column_variant() {
        for has_workspace_name in [false, true] {
            for has_sync_pid in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let path = database_path(&directory);
                let mut connection = create_schema0(&path, has_workspace_name, has_sync_pid).await;

                initialize(&mut connection, &path).await.unwrap();

                assert_eq!(schema_version(&mut connection).await.unwrap(), 1);
                let instance_columns = table_columns(&mut connection, "instances").await.unwrap();
                for (column, _) in INSTANCE_COLUMNS_V1 {
                    assert!(
                        instance_columns.contains(&(*column).to_string()),
                        "missing {column} for workspace_name={has_workspace_name}, pid={has_sync_pid}"
                    );
                }
                let sync_columns = table_columns(&mut connection, "sync_sessions").await.unwrap();
                assert!(sync_columns.contains(&"pid".to_string()));
                let port_columns = table_columns(&mut connection, "port_mappings").await.unwrap();
                for (column, _) in PORT_MAPPING_COLUMNS_V1 {
                    assert!(port_columns.contains(&(*column).to_string()));
                }
                assert_eq!(backups(&directory).len(), 1);
            }
        }
    }

    #[tokio::test]
    async fn migration_backup_is_a_restorable_schema0_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let path = database_path(&directory);
        let mut connection = create_schema0(&path, true, true).await;
        insert_legacy_instance(
            &mut connection,
            "legacy",
            workspace.path(),
            Some("workspace"),
            "remote",
            "stopped",
            10,
        )
        .await;

        initialize(&mut connection, &path).await.unwrap();
        connection.close().await.unwrap();

        let backup_path = backups(&directory).pop().unwrap();
        let mut backup = connect(&backup_path).await;
        assert_eq!(schema_version(&mut backup).await.unwrap(), 0);
        assert!(!table_columns(&mut backup, "instances")
            .await
            .unwrap()
            .contains(&"session_key".to_string()));
        let id: String =
            sqlx::query_scalar("SELECT id FROM instances").fetch_one(&mut backup).await.unwrap();
        assert_eq!(id, "legacy");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&backup_path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[tokio::test]
    async fn active_local_rows_use_canonical_worktree_identity_without_workspace_name() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let other_workspace = tempfile::tempdir().unwrap();
        let path = database_path(&directory);
        let mut connection = create_schema0(&path, true, true).await;
        insert_legacy_instance(
            &mut connection,
            "local-older",
            workspace.path(),
            None,
            "local",
            "running",
            10,
        )
        .await;
        insert_legacy_instance(
            &mut connection,
            "local-newer",
            workspace.path(),
            None,
            "local",
            "starting",
            20,
        )
        .await;
        insert_legacy_instance(
            &mut connection,
            "local-other",
            other_workspace.path(),
            None,
            "local",
            "running",
            30,
        )
        .await;

        initialize(&mut connection, &path).await.unwrap();

        let older = sqlx::query(
            "SELECT status, is_quarantined, superseded_by, session_key, planned_workspace_name
             FROM instances WHERE id = 'local-older'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert_eq!(older.get::<String, _>("status"), "stopped");
        assert_eq!(older.get::<i64, _>("is_quarantined"), 0);
        assert_eq!(older.get::<String, _>("superseded_by"), "local-newer");
        let shared_key = older.get::<String, _>("session_key");
        assert!(older.get::<Option<String>, _>("planned_workspace_name").is_none());

        let newer = sqlx::query(
            "SELECT status, is_quarantined, session_key FROM instances WHERE id = 'local-newer'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert_eq!(newer.get::<String, _>("status"), "starting");
        assert_eq!(newer.get::<i64, _>("is_quarantined"), 0);
        assert_eq!(newer.get::<String, _>("session_key"), shared_key);

        let other = sqlx::query(
            "SELECT status, is_quarantined, session_key FROM instances WHERE id = 'local-other'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert_eq!(other.get::<String, _>("status"), "running");
        assert_eq!(other.get::<i64, _>("is_quarantined"), 0);
        assert_ne!(other.get::<String, _>("session_key"), shared_key);
    }

    #[tokio::test]
    async fn duplicate_active_rows_are_deduplicated_and_ambiguous_active_rows_quarantined() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let path = database_path(&directory);
        let mut connection = create_schema0(&path, true, true).await;
        insert_legacy_instance(
            &mut connection,
            "older",
            workspace.path(),
            Some("workspace"),
            "remote",
            "running",
            10,
        )
        .await;
        insert_legacy_instance(
            &mut connection,
            "newer",
            workspace.path(),
            Some("workspace"),
            "remote",
            "starting",
            20,
        )
        .await;
        insert_legacy_instance(
            &mut connection,
            "ambiguous-active",
            workspace.path(),
            None,
            "remote",
            "running",
            30,
        )
        .await;
        insert_legacy_instance(
            &mut connection,
            "ambiguous-terminal",
            workspace.path(),
            None,
            "remote",
            "stopped",
            40,
        )
        .await;

        initialize(&mut connection, &path).await.unwrap();

        let older = sqlx::query(
            "SELECT status, superseded_by, failure_reason, session_key FROM instances WHERE id = 'older'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert_eq!(older.get::<String, _>("status"), "stopped");
        assert_eq!(older.get::<String, _>("superseded_by"), "newer");
        assert_eq!(older.get::<String, _>("failure_reason"), "duplicate_active_session");
        assert!(older.get::<Option<String>, _>("session_key").is_some());

        let newer_status: String =
            sqlx::query_scalar("SELECT status FROM instances WHERE id = 'newer'")
                .fetch_one(&mut connection)
                .await
                .unwrap();
        assert_eq!(newer_status, "starting");

        let quarantined = sqlx::query(
            "SELECT status, is_quarantined, quarantine_reason, session_key FROM instances WHERE id = 'ambiguous-active'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert_eq!(quarantined.get::<String, _>("status"), "quarantined");
        assert_eq!(quarantined.get::<i64, _>("is_quarantined"), 1);
        assert_eq!(
            quarantined.get::<String, _>("quarantine_reason"),
            "legacy_session_identity_ambiguous"
        );
        assert!(quarantined.get::<Option<String>, _>("session_key").is_none());

        let terminal = sqlx::query(
            "SELECT status, is_quarantined, session_key FROM instances WHERE id = 'ambiguous-terminal'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert_eq!(terminal.get::<String, _>("status"), "stopped");
        assert_eq!(terminal.get::<i64, _>("is_quarantined"), 0);
        assert!(terminal.get::<Option<String>, _>("session_key").is_none());
    }

    #[tokio::test]
    async fn migration_failure_rolls_back_every_schema_change() {
        let directory = tempfile::tempdir().unwrap();
        let path = database_path(&directory);
        let mut connection = connect(&path).await;
        sqlx::query("CREATE TABLE instances (legacy_only TEXT)")
            .execute(&mut connection)
            .await
            .unwrap();

        let error = initialize(&mut connection, &path).await.unwrap_err();

        assert!(error.to_string().contains("no such column"));
        assert_eq!(schema_version(&mut connection).await.unwrap(), 0);
        assert_eq!(table_columns(&mut connection, "instances").await.unwrap(), vec!["legacy_only"]);
        assert_eq!(backups(&directory).len(), 1);
    }

    #[tokio::test]
    async fn future_schema_is_refused_without_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = database_path(&directory);
        let mut connection = connect(&path).await;
        sqlx::query("CREATE TABLE future_marker (value TEXT)")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("PRAGMA user_version = 2").execute(&mut connection).await.unwrap();

        let error = initialize(&mut connection, &path).await.unwrap_err();

        assert!(error.to_string().contains("unsupported schema version 2"));
        assert_eq!(schema_version(&mut connection).await.unwrap(), 2);
        assert!(has_table(&mut connection, "future_marker").await.unwrap());
        assert!(!has_table(&mut connection, "instances").await.unwrap());
        assert!(backups(&directory).is_empty());
    }

    #[tokio::test]
    async fn schema_v1_remains_writable_by_schema0_insert_statements() {
        let directory = tempfile::tempdir().unwrap();
        let path = database_path(&directory);
        let mut connection = connect(&path).await;
        initialize(&mut connection, &path).await.unwrap();

        sqlx::query(
            "INSERT INTO instances (
                id, workspace_path, workspace_name, branch, profile, mode, status,
                port_base, created_at, updated_at
             ) VALUES ('old-writer', '/repo', 'workspace', 'main', 'test', 'remote',
                       'running', 0, 1, 1)",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO port_mappings (
                devdock_id, service_name, port_type, local_port, remote_port, process_pid
             ) VALUES ('old-writer', 'tilt', 'TiltUI', 10350, 10350, 42)",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sync_sessions (
                devdock_id, workspace_name, pid, full_mode, last_sync_time, status
             ) VALUES ('old-writer', 'workspace', 43, 0, 1, 'running')",
        )
        .execute(&mut connection)
        .await
        .unwrap();

        let identity = sqlx::query(
            "SELECT session_key, is_quarantined FROM instances WHERE id = 'old-writer'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert!(identity.get::<Option<String>, _>("session_key").is_none());
        assert_eq!(identity.get::<i64, _>("is_quarantined"), 0);

        let ownership = sqlx::query(
            "SELECT workspace_id, owner_kind, process_start_time, command_fingerprint
             FROM port_mappings WHERE devdock_id = 'old-writer'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert!(ownership.get::<Option<String>, _>("workspace_id").is_none());
        assert!(ownership.get::<Option<String>, _>("owner_kind").is_none());
        assert!(ownership.get::<Option<i64>, _>("process_start_time").is_none());
        assert!(ownership.get::<Option<String>, _>("command_fingerprint").is_none());
    }
}
