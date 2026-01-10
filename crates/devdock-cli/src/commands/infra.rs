//! Infrastructure setup commands.
//!
//! Consolidates all infra setup into the Rust binary, replacing:
//! - devdock/config/infra/sql/scripts/sql_hydrator.sh
//! - devdock/config/infra/sql/util/util.py
//! - devdock/config/infra/opensearch/scripts/copy_indexes.py
//!
//! Usage:
//!   devdock infra sql setup --project dev-sandbox-334901 --db-tables "keyval:KeyVal"
//!   devdock infra opensearch copy --project dev-sandbox-334901 --indexes "index-1"

use crate::output;
use anyhow::{Context, Result};
use devdock_config::gcs_storage::DynamicIniLoader;
use devdock_config::opensearch_import::OpenSearchCopier;
use devdock_config::sql_import::SqlOperationsManager;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// State tracking for SQL hydration
struct SqlState {
    state_dir: PathBuf,
}

impl SqlState {
    fn new() -> Result<Self> {
        let home = std::env::var("HOME")?;
        let state_dir = PathBuf::from(home).join(".devdock/sql");
        std::fs::create_dir_all(&state_dir)?;
        Ok(Self { state_dir })
    }

    fn read_file(&self, name: &str) -> String {
        let path = self.state_dir.join(name);
        std::fs::read_to_string(&path).unwrap_or_default()
    }

    fn write_file(&self, name: &str, content: &str) -> Result<()> {
        let path = self.state_dir.join(name);
        std::fs::write(&path, content)?;
        Ok(())
    }

    fn prev_project(&self) -> String {
        self.read_file("project.txt")
    }

    fn prev_tables(&self) -> String {
        self.read_file("copy_tables.txt")
    }

    fn prev_full_cleanup(&self) -> bool {
        self.read_file("full_cleanup.txt").trim() == "true"
    }

    fn set_project(&self, project: &str) -> Result<()> {
        self.write_file("project.txt", project)
    }

    fn set_tables(&self, tables: &str) -> Result<()> {
        self.write_file("copy_tables.txt", tables)
    }

    fn set_full_cleanup(&self, value: bool) -> Result<()> {
        self.write_file("full_cleanup.txt", if value { "true" } else { "false" })
    }

    fn scripts_dir(&self) -> PathBuf {
        self.state_dir.join("scripts")
    }

    fn clear_scripts(&self) -> Result<()> {
        let scripts = self.scripts_dir();
        if scripts.exists() {
            std::fs::remove_dir_all(&scripts)?;
        }
        Ok(())
    }
}

/// Wait for MySQL container to be healthy
fn wait_for_mysql(container: &str, timeout: Duration) -> Result<()> {
    output::info(&format!("Waiting for {container} to be healthy..."));

    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        let output = Command::new("docker")
            .args(["container", "inspect", "--format={{.State.Health.Status}}", container])
            .output()?;

        let status = String::from_utf8_lossy(&output.stdout);
        if status.trim() == "healthy" {
            output::success(&format!("{container} is healthy"));
            return Ok(());
        }

        std::thread::sleep(Duration::from_secs(2));
    }

    anyhow::bail!("Timeout waiting for {container} to be healthy")
}

/// Execute SQL command in container
fn docker_mysql_exec(container: &str, sql: &str) -> Result<()> {
    let status = Command::new("docker")
        .args(["exec", container, "mysql", "-u", "root", "-e", sql])
        .status()?;

    if !status.success() {
        anyhow::bail!("MySQL command failed");
    }
    Ok(())
}

/// Execute SQL file in container (with GTID stripping)
fn docker_mysql_import(container: &str, sql_file: &PathBuf) -> Result<()> {
    // Read file and strip GTID statements
    let content = std::fs::read_to_string(sql_file)?;

    // Strip GTID statements using regex
    let re = regex::Regex::new(r"SET @@GLOBAL\.GTID_[A-Z_]+=[^;]*;").unwrap();
    let cleaned = re.replace_all(&content, "");

    // Pipe to mysql
    let mut child = Command::new("docker")
        .args(["exec", "-i", container, "mysql", "-u", "root"])
        .stdin(std::process::Stdio::piped())
        .spawn()?;

    if let Some(ref mut stdin) = child.stdin {
        use std::io::Write;
        stdin.write_all(cleaned.as_bytes())?;
    }

    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("Failed to import SQL file: {sql_file:?}");
    }

    Ok(())
}

/// Get SQL scripts path for a project
fn get_sql_scripts_path(devdock_root: &Path, project: &str) -> PathBuf {
    let base = devdock_root.join("java/com/askscio/storage/sql/scripts");

    match project {
        "scio-apps" | "glean-apps-canary" => base.join("scio_apps"),
        "scio-engineering" => base.join("scio_engineering"),
        _ => base,
    }
}

/// Full SQL setup - replaces sql_hydrator.sh
#[allow(clippy::too_many_arguments)]
pub async fn sql_setup(
    project: &str,
    platform: &str,
    region: &str,
    db_tables: &str,
    mode: &str,
    force_copy: bool,
    full_cleanup: bool,
    copy_keyval: bool,
) -> Result<()> {
    output::info(&format!("Setting up SQL for project {project} ({platform})"));

    let devdock_root =
        std::env::var("DEVDOCK_ROOT").context("DEVDOCK_ROOT environment variable not set")?;
    let devdock_root = PathBuf::from(&devdock_root);

    // Determine container name
    let container =
        if project == "glean-apps-canary" { "devdock-mysql-gac" } else { "devdock-mysql" };

    // Wait for MySQL
    wait_for_mysql(container, Duration::from_secs(120))?;

    // State tracking
    let state = SqlState::new()?;
    let prev_project = state.prev_project();
    let prev_tables = state.prev_tables();
    let prev_full_cleanup = state.prev_full_cleanup();

    // Determine what tables to copy
    let mut tables_to_copy = db_tables.to_string();
    let mut update_project = false;
    let mut update_tables = false;

    // Add keyval if not present and copy_keyval is enabled
    if !force_copy && copy_keyval && !tables_to_copy.contains("keyval:KeyVal") {
        if !tables_to_copy.is_empty() {
            tables_to_copy.push(';');
        }
        tables_to_copy.push_str("keyval:KeyVal");
    }

    // Load INI config (default.ini merged with dynamic.ini from GCS)
    // The dynamic.ini contains project-specific values like SQL instance names
    let ini_loader = DynamicIniLoader::new(project, devdock_root.clone())?;
    let config = ini_loader.load_merged_config().await?;
    let manager = SqlOperationsManager::new(project, platform, region, config);

    // Handle project change
    if project != prev_project && !prev_project.is_empty() {
        output::info(&format!("Project changed from {prev_project} to {project}, cleaning up..."));
        state.clear_scripts()?;
        manager.clean_sql("")?;
        update_project = true;
        update_tables = true;
    }
    // Handle full cleanup
    else if full_cleanup && !prev_full_cleanup {
        output::info("Full cleanup requested, cleaning up...");
        state.clear_scripts()?;
        manager.clean_sql("")?;
        state.set_full_cleanup(true)?;
        update_project = true;
        update_tables = true;
    }
    // Handle table comparison
    else {
        let mut prev_sorted: Vec<&str> = prev_tables.split(';').filter(|s| !s.is_empty()).collect();
        let mut curr_sorted: Vec<&str> =
            tables_to_copy.split(';').filter(|s| !s.is_empty()).collect();
        prev_sorted.sort_unstable();
        curr_sorted.sort_unstable();

        if prev_sorted == curr_sorted && !force_copy {
            if copy_keyval {
                tables_to_copy = "keyval:KeyVal".to_string();
                output::info("Tables match, refreshing keyval only");
            } else {
                tables_to_copy.clear();
                output::info("Tables match, skipping copy");
            }
        } else {
            update_tables = true;
        }
    }

    // Reset MySQL master
    output::info("Resetting MySQL master...");
    docker_mysql_exec(container, "RESET MASTER;")?;
    docker_mysql_exec(container, "SET GLOBAL max_allowed_packet=512*1024*1024;")?;

    // Copy and run schema scripts
    let source_scripts = get_sql_scripts_path(&devdock_root, project);
    let target_scripts = state.scripts_dir().join(project);
    std::fs::create_dir_all(&target_scripts)?;

    if source_scripts.exists() {
        output::info(&format!("Copying SQL scripts from {source_scripts:?}"));
        for entry in std::fs::read_dir(&source_scripts)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                let dest = target_scripts.join(path.file_name().unwrap());
                std::fs::copy(&path, &dest)?;
            }
        }
    }

    // Run schema files
    output::info("Running schema hydration...");
    let mut sql_files: Vec<_> = std::fs::read_dir(&target_scripts)?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "sql") && !p.to_string_lossy().ends_with(".template")
        })
        .collect();
    sql_files.sort();

    for sql_file in &sql_files {
        output::info(&format!("⏩ Executing {:?}", sql_file.file_name().unwrap()));
        docker_mysql_import(container, sql_file)?;
        output::success(&format!("✅ Executed {:?}", sql_file.file_name().unwrap()));
    }

    output::success("Schema hydration complete");

    // Export data from cloud
    if !tables_to_copy.is_empty() {
        output::info(&format!("Exporting tables: {tables_to_copy}"));

        let export_dir = state.scripts_dir();
        std::fs::create_dir_all(&export_dir)?;

        for entry in tables_to_copy.split(';') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            output::info(&format!("Processing: {entry}"));
            manager.export_from_cloud(entry, &export_dir, mode).await?;
        }

        // Import exported dumps
        output::info("Importing exported data...");
        let mut dump_files: Vec<_> = std::fs::read_dir(&export_dir)?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|e| e == "sql")
                    && !p.to_string_lossy().ends_with(".template")
            })
            .collect();
        dump_files.sort();

        for dump_file in &dump_files {
            output::info(&format!("⏩ Importing {:?}", dump_file.file_name().unwrap()));
            docker_mysql_import(container, dump_file)?;
            output::success(&format!("✅ Imported {:?}", dump_file.file_name().unwrap()));
        }
    }

    // Update state
    if update_tables {
        state.set_tables(&tables_to_copy)?;
    }
    if update_project {
        state.set_project(project)?;
    }

    // Cleanup
    state.clear_scripts()?;

    output::success("✅ SQL setup complete");
    Ok(())
}

/// SQL cleanup
pub async fn sql_clean(project: &str, platform: &str, region: &str, db_tables: &str) -> Result<()> {
    output::info(&format!("Cleaning SQL for project {project}"));

    let devdock_root =
        std::env::var("DEVDOCK_ROOT").context("DEVDOCK_ROOT environment variable not set")?;
    let devdock_root = PathBuf::from(devdock_root);

    // Load merged config (default.ini + dynamic.ini from GCS)
    let ini_loader = DynamicIniLoader::new(project, devdock_root)?;
    let config = ini_loader.load_merged_config().await?;

    let manager = SqlOperationsManager::new(project, platform, region, config);
    manager.clean_sql(db_tables)?;

    output::success("SQL cleanup complete");
    Ok(())
}

/// OpenSearch setup - copy indexes from remote
pub async fn opensearch_copy(
    project: &str,
    indexes: &str,
    query: &str,
    max_docs: usize,
) -> Result<()> {
    output::info(&format!("Copying OpenSearch indexes for project {project}: {indexes}"));

    let copier = OpenSearchCopier::new(project).await?;

    let index_list: Vec<String> =
        indexes.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();

    if index_list.is_empty() {
        output::warn("No indexes specified");
        return Ok(());
    }

    let results = copier.copy_indexes(&index_list, query, max_docs)?;

    for (index, count) in &results {
        output::success(&format!("Copied {count} documents for {index}"));
    }

    output::success(&format!("✅ Completed copying {} index(es)", results.len()));

    Ok(())
}

/// Check OpenSearch health
pub async fn opensearch_health() -> Result<()> {
    output::info("Checking local OpenSearch health...");

    let client = reqwest::Client::new();
    let response = client.get("http://localhost:9200/_cluster/health").send().await?;

    if response.status().is_success() {
        let health: serde_json::Value = response.json().await?;
        let status = health.get("status").and_then(|s| s.as_str()).unwrap_or("unknown");

        match status {
            "green" => output::success("OpenSearch cluster is healthy (green)"),
            "yellow" => output::warn("OpenSearch cluster is degraded (yellow)"),
            "red" => output::warn("OpenSearch cluster is unhealthy (red)"),
            _ => output::info(&format!("OpenSearch cluster status: {status}")),
        }
    } else {
        output::warn("Failed to get cluster health");
    }

    Ok(())
}
