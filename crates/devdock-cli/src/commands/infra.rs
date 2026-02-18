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
use devdock_config::ini_parser::ParsedIniConfig;
use devdock_config::opensearch_import::OpenSearchCopier;
use devdock_config::s3_storage::S3DynamicIniLoader;
use devdock_config::sql_import::SqlOperationsManager;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// Load merged INI config (default.ini + dynamic.ini) from the appropriate cloud storage
/// based on platform (GCS for Google, S3 for AWS)
async fn load_platform_config(
    project: &str,
    platform: &str,
    region: &str,
    devdock_root: PathBuf,
) -> Result<ParsedIniConfig> {
    match platform {
        "aws" => {
            // For AWS, we need the AWS account ID (not project name) for S3 bucket
            // The bucket is s3://config-{account_id}/dynamic.ini
            let account_id = get_aws_account_id().await?;
            println!("     📦 Loading config from S3 (account: {account_id}, region: {region})");
            let ini_loader =
                S3DynamicIniLoader::new(&account_id, devdock_root, Some(region.to_string()))?;
            ini_loader.load_merged_config().await
        }
        _ => {
            // Default to GCS for Google, Azure, and other platforms
            let ini_loader = DynamicIniLoader::new(project, devdock_root).await?;
            ini_loader.load_merged_config().await
        }
    }
}

/// Get AWS account ID using STS
async fn get_aws_account_id() -> Result<String> {
    let output = tokio::process::Command::new("aws")
        .args(["sts", "get-caller-identity", "--query", "Account", "--output", "text"])
        .output()
        .await
        .context("Failed to run aws sts get-caller-identity")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to get AWS account ID: {stderr}");
    }

    let account_id = String::from_utf8(output.stdout)?.trim().to_string();
    if account_id.is_empty() {
        anyhow::bail!("AWS account ID is empty - check AWS credentials");
    }

    Ok(account_id)
}

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

    /// Get the path to the MySQL seed cache directory
    fn seed_cache_path(&self) -> PathBuf {
        self.state_dir.join("mysql-seed")
    }

    /// Check if the seed cache is valid (exists and not expired)
    /// Cache validity: 4 hours (matches shell script behavior)
    fn is_seed_cache_valid(&self, max_age_hours: u64) -> bool {
        let timestamp_file = self.state_dir.join("seed_timestamp.txt");
        if !timestamp_file.exists() {
            return false;
        }

        let seed_dir = self.seed_cache_path();
        if !seed_dir.exists() || !seed_dir.is_dir() {
            return false;
        }

        // Check if directory has content
        if std::fs::read_dir(&seed_dir).map(|mut d| d.next().is_none()).unwrap_or(true) {
            return false;
        }

        // Check timestamp
        if let Ok(content) = std::fs::read_to_string(&timestamp_file) {
            if let Ok(timestamp) = content.trim().parse::<i64>() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let age_hours = (now - timestamp) / 3600;
                return age_hours < max_age_hours as i64;
            }
        }

        false
    }

    /// Set the seed cache timestamp to now
    fn set_seed_timestamp(&self) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.write_file("seed_timestamp.txt", &now.to_string())
    }

    /// Clear the seed cache directory
    fn clear_seed_cache(&self) -> Result<()> {
        let seed_dir = self.seed_cache_path();
        if seed_dir.exists() {
            std::fs::remove_dir_all(&seed_dir)?;
        }
        let timestamp_file = self.state_dir.join("seed_timestamp.txt");
        if timestamp_file.exists() {
            std::fs::remove_file(&timestamp_file)?;
        }
        Ok(())
    }
}

/// Wait for MySQL container to be healthy and accepting connections.
///
/// Docker's health status can briefly report "healthy" before MySQL's socket
/// is actually ready. After the health check passes, we verify connectivity
/// by running a trivial query via `docker exec`.
fn wait_for_mysql(container: &str, timeout: Duration) -> Result<()> {
    output::info(&format!("Waiting for {container} to be healthy..."));

    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        let output = Command::new("docker")
            .args(["container", "inspect", "--format={{.State.Health.Status}}", container])
            .output()?;

        let status = String::from_utf8_lossy(&output.stdout);
        if status.trim() == "healthy" {
            break;
        }

        std::thread::sleep(Duration::from_secs(2));
    }

    if start.elapsed() >= timeout {
        anyhow::bail!("Timeout waiting for {container} to be healthy");
    }

    // Docker health check passed, but MySQL may not be accepting connections yet.
    // Verify with an actual connectivity probe before proceeding.
    output::info(&format!("Verifying {container} accepts connections..."));
    while start.elapsed() < timeout {
        let probe = Command::new("docker")
            .args(["exec", container, "mysql", "-u", "root", "-e", "SELECT 1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        if let Ok(s) = probe {
            if s.success() {
                output::success(&format!("{container} is healthy"));
                return Ok(());
            }
        }

        std::thread::sleep(Duration::from_secs(2));
    }

    anyhow::bail!("Timeout: {container} reported healthy but MySQL is not accepting connections")
}

/// Execute SQL command in container with retry logic for transient connection failures
fn docker_mysql_exec(container: &str, sql: &str) -> Result<()> {
    let max_retries = 5;
    let mut last_error = None;

    for attempt in 0..max_retries {
        let output = Command::new("docker")
            .args(["exec", container, "mysql", "-u", "root", "-e", sql])
            .output()?;

        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        // Check for socket/connection errors that are worth retrying
        // ERROR 2002: Can't connect to local MySQL server through socket
        // ERROR 2003: Can't connect to MySQL server
        if stderr.contains("ERROR 2002")
            || stderr.contains("ERROR 2003")
            || stderr.contains("Can't connect")
        {
            last_error = Some(format!("MySQL connection failed: {stderr}"));
            if attempt < max_retries - 1 {
                let delay = 2u64.pow(attempt as u32); // 1s, 2s, 4s, 8s
                output::warn(&format!(
                    "MySQL not ready, retrying in {delay}s ({}/{max_retries})...",
                    attempt + 1
                ));
                std::thread::sleep(Duration::from_secs(delay));
                continue;
            }
        } else {
            // Non-retryable error
            anyhow::bail!("MySQL command failed: {stderr}");
        }
    }

    anyhow::bail!(last_error.unwrap_or_else(|| "MySQL command failed after retries".to_string()))
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

/// Set proper permissions on the MySQL seed directory
///
/// Matches shell script behavior from devdock.sh:
/// - chmod 755 for directories
/// - chmod 644 for files
/// - chmod 600 for secure files (*.pem, auto.cnf, binlog.*)
/// - chown 999:999 (MySQL user) for container access
fn set_seed_permissions(seed_dir: &Path) -> Result<()> {
    output::info("Setting proper permissions for MySQL seed data...");

    // Set directory permissions (755)
    let _ = Command::new("find")
        .args([seed_dir.to_str().unwrap(), "-type", "d", "-exec", "chmod", "755", "{}", ";"])
        .status();

    // Set file permissions (644)
    let _ = Command::new("find")
        .args([seed_dir.to_str().unwrap(), "-type", "f", "-exec", "chmod", "644", "{}", ";"])
        .status();

    // Set secure file permissions (600) for *.pem, auto.cnf, binlog.*
    for pattern in &["*.pem", "auto.cnf", "binlog.*"] {
        let _ = Command::new("find")
            .args([
                seed_dir.to_str().unwrap(),
                "-name",
                pattern,
                "-exec",
                "chmod",
                "600",
                "{}",
                ";",
            ])
            .status();
    }

    // Change ownership to MySQL user (999:999) for container access
    // This requires sudo, so we try it but don't fail if it doesn't work
    if Command::new("which").arg("sudo").status().map(|s| s.success()).unwrap_or(false) {
        let _ = Command::new("sudo")
            .args(["chown", "-R", "999:999", seed_dir.to_str().unwrap()])
            .status();
    }

    output::success("Permissions set for MySQL seed data");
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
    use_mysql_seed: bool,
    use_full_instance: bool,
    use_gac_container: bool,
) -> Result<()> {
    // Handle seed mode (fastest startup)
    if use_mysql_seed {
        return sql_setup_with_seed(
            project,
            platform,
            region,
            db_tables,
            mode,
            force_copy,
            use_gac_container,
        )
        .await;
    }

    // Handle full instance mode
    if use_full_instance {
        return sql_setup_with_full_instance(
            project,
            platform,
            region,
            db_tables,
            mode,
            use_gac_container,
        )
        .await;
    }

    output::info(&format!("Setting up SQL for project {project} ({platform})"));

    let devdock_root =
        std::env::var("DEVDOCK_ROOT").context("DEVDOCK_ROOT environment variable not set")?;
    let devdock_root = PathBuf::from(&devdock_root);

    // Container name: devdock-mysql for main setup, devdock-mysql-gac for secondary GAC setup
    let container = if use_gac_container { "devdock-mysql-gac" } else { "devdock-mysql" };

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

    // Load INI config (default.ini merged with dynamic.ini from cloud storage)
    // Uses GCS for Google, S3 for AWS - the dynamic.ini contains project-specific values like SQL instance names
    let config = load_platform_config(project, platform, region, devdock_root.clone()).await?;
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

    // Export data from cloud (parallelized for speed)
    if !tables_to_copy.is_empty() {
        output::info(&format!("Exporting tables: {tables_to_copy}"));

        let export_dir = state.scripts_dir();
        std::fs::create_dir_all(&export_dir)?;

        // Collect all entries to export
        let entries: Vec<&str> =
            tables_to_copy.split(';').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();

        // Separate fast exports (GCS downloads) from slow exports (gcloud sql)
        // Fast exports can be fully parallelized, slow exports are rate-limited
        let fast_export_config = manager.get_fast_export_config_public();
        let mut fast_entries = Vec::new();
        let mut slow_entries = Vec::new();

        for entry in &entries {
            let (db, _tables, query) = devdock_config::ini_parser::parse_db_tables_query(entry);
            let is_fast = query.is_none()
                && fast_export_config.projects.contains(project)
                && fast_export_config.dbs.contains(&db)
                && mode == "fast";

            if is_fast {
                fast_entries.push(*entry);
            } else {
                slow_entries.push(*entry);
            }
        }

        // Run exports with optimal parallelism:
        // - Fast exports (GCS downloads): fully parallel
        // - Slow exports (gcloud sql): SEQUENTIAL (Cloud SQL only allows one export per instance)
        output::info(&format!(
            "🚀 Running {} fast exports (parallel) + {} slow exports (sequential)",
            fast_entries.len(),
            slow_entries.len()
        ));

        // Run fast exports in parallel (GCS downloads are independent)
        let fast_future = async {
            if fast_entries.is_empty() {
                return Ok::<(), anyhow::Error>(());
            }

            let fast_futures: Vec<_> = fast_entries
                .iter()
                .map(|entry| {
                    let manager = &manager;
                    let export_dir = &export_dir;
                    async move {
                        output::info(&format!("  ⚡ Fast: {entry}"));
                        let result = manager.export_from_cloud(entry, export_dir, mode).await;
                        if result.is_ok() {
                            output::success(&format!("  ✅ Fast done: {entry}"));
                        }
                        result
                    }
                })
                .collect();

            let results = futures::future::join_all(fast_futures).await;
            for result in results {
                result?;
            }
            output::success(&format!("✅ {} fast exports complete", fast_entries.len()));
            Ok(())
        };

        // Run slow exports sequentially with retry logic
        // Cloud SQL only allows one export operation per instance at a time
        let slow_future = async {
            if slow_entries.is_empty() {
                return Ok::<(), anyhow::Error>(());
            }

            for entry in &slow_entries {
                output::info(&format!("  🐢 Slow: {entry}"));

                // Retry logic with exponential backoff for 409 conflicts
                let max_retries = 3;
                let mut retry_count = 0;
                let mut last_error = None;

                while retry_count < max_retries {
                    match manager.export_from_cloud(entry, &export_dir, mode).await {
                        Ok(()) => {
                            output::success(&format!("  ✅ Slow done: {entry}"));
                            last_error = None;
                            break;
                        }
                        Err(e) => {
                            let error_str = e.to_string();
                            // Check if it's a 409 conflict (another operation in progress)
                            if error_str.contains("409")
                                || error_str.contains("already in progress")
                            {
                                retry_count += 1;
                                if retry_count < max_retries {
                                    let delay = 2u64.pow(retry_count as u32) * 5; // 10s, 20s, 40s
                                    output::warn(&format!(
                                        "  ⏳ Cloud SQL busy, retrying in {delay}s ({retry_count}/{max_retries})..."
                                    ));
                                    tokio::time::sleep(Duration::from_secs(delay)).await;
                                } else {
                                    last_error = Some(e);
                                }
                            } else {
                                // Non-retryable error
                                last_error = Some(e);
                                break;
                            }
                        }
                    }
                }

                if let Some(e) = last_error {
                    return Err(e);
                }
            }

            output::success(&format!("✅ {} slow exports complete", slow_entries.len()));
            Ok(())
        };

        // Run fast and slow exports concurrently
        // Fast exports run in parallel among themselves
        // Slow exports run sequentially among themselves (but concurrent with fast)
        let (fast_result, slow_result) = futures::future::join(fast_future, slow_future).await;
        fast_result?;
        slow_result?;

        output::success(&format!(
            "✅ All {} exports complete",
            fast_entries.len() + slow_entries.len()
        ));

        // Import exported dumps (sequential - MySQL handles one write connection best)
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

/// SQL setup using pre-seeded MySQL data directory (fastest startup)
///
/// This downloads a pre-built MySQL data directory from GCS and uses it directly,
/// avoiding the need for schema hydration and data import.
///
/// GCS path: gs://sql-{project}/devdock/devdock-seeds/latest.tar.zst
async fn sql_setup_with_seed(
    project: &str,
    platform: &str,
    region: &str,
    db_tables: &str,
    mode: &str,
    force_copy: bool,
    use_gac_container: bool,
) -> Result<()> {
    output::info(&format!("Setting up SQL with MySQL seed for project {project}"));

    let state = SqlState::new()?;
    let seed_cache_max_age_hours = 4; // Match shell script behavior

    // Check if we have a valid cached seed
    if !force_copy && state.is_seed_cache_valid(seed_cache_max_age_hours) {
        output::success("Using cached MySQL seed (less than 4 hours old)");
    } else {
        // Download fresh seed
        output::info("Downloading MySQL seed from GCS...");

        let devdock_root =
            std::env::var("DEVDOCK_ROOT").context("DEVDOCK_ROOT environment variable not set")?;
        let devdock_root = PathBuf::from(&devdock_root);

        // Load config using platform-appropriate storage (GCS for Google, S3 for AWS)
        let config = load_platform_config(project, platform, region, devdock_root).await?;
        let manager = SqlOperationsManager::new(project, platform, region, config);

        // Clear old cache
        state.clear_seed_cache()?;

        // Download and extract seed
        let seed_dir = state.seed_cache_path();
        std::fs::create_dir_all(&seed_dir)?;

        manager
            .download_mysql_seed(&seed_dir)
            .await
            .map_err(|e| {
                output::warn(&format!("Seed download failed: {e}"));
                output::info("Falling back to table-by-table import...");
                e
            })
            .context("Failed to download MySQL seed. Use --use-full-instance or remove the flag to use table-by-table import.")?;

        // Set proper permissions on the seed directory (matches devdock.sh behavior)
        set_seed_permissions(&seed_dir)?;

        state.set_seed_timestamp()?;
        output::success("MySQL seed downloaded and extracted");
    }

    // Set DEVDOCK_MYSQL_DATA_DIR environment variable for Docker to use
    let seed_dir = state.seed_cache_path();
    output::info(&format!("MySQL data directory: {}", seed_dir.display()));
    std::env::set_var("DEVDOCK_MYSQL_DATA_DIR", &seed_dir);

    // If additional tables were specified, overlay them on top of seed
    if !db_tables.is_empty() {
        output::info(&format!("Applying incremental overlay: {db_tables}"));

        // Wait for MySQL to be ready with the seed data
        // Container name: devdock-mysql for main setup, devdock-mysql-gac for secondary GAC setup
        let container = if use_gac_container { "devdock-mysql-gac" } else { "devdock-mysql" };
        wait_for_mysql(container, Duration::from_secs(120))?;

        // Reset master and import overlay tables
        docker_mysql_exec(container, "RESET MASTER;")?;
        docker_mysql_exec(container, "SET GLOBAL max_allowed_packet=512*1024*1024;")?;

        let devdock_root =
            std::env::var("DEVDOCK_ROOT").context("DEVDOCK_ROOT environment variable not set")?;
        let devdock_root = PathBuf::from(&devdock_root);
        let config = load_platform_config(project, platform, region, devdock_root).await?;
        let manager = SqlOperationsManager::new(project, platform, region, config);

        let export_dir = state.scripts_dir();
        std::fs::create_dir_all(&export_dir)?;

        for entry in db_tables.split(';') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            output::info(&format!("Processing overlay: {entry}"));
            manager.export_from_cloud(entry, &export_dir, mode).await?;
        }

        // Import exported dumps
        output::info("Importing overlay data...");
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

        state.clear_scripts()?;
    }

    output::success("✅ SQL setup with seed complete");
    Ok(())
}

/// SQL setup using full instance dump
///
/// Downloads and imports a complete MySQL dump file instead of table-by-table.
/// This is faster than table-by-table for large databases but slower than seed mode.
///
/// GCS path: gs://sql-{project}/devdock/{sql_instance_id}-full.sql.gz
///
/// Matches shell script behavior from sql_hydrator.sh lines 289-318
async fn sql_setup_with_full_instance(
    project: &str,
    platform: &str,
    region: &str,
    db_tables: &str,
    mode: &str,
    use_gac_container: bool,
) -> Result<()> {
    output::info(&format!("Setting up SQL with full instance dump for project {project}"));

    let devdock_root =
        std::env::var("DEVDOCK_ROOT").context("DEVDOCK_ROOT environment variable not set")?;
    let devdock_root = PathBuf::from(&devdock_root);

    // Container name: devdock-mysql for main setup, devdock-mysql-gac for secondary GAC setup
    let container = if use_gac_container { "devdock-mysql-gac" } else { "devdock-mysql" };

    // Wait for MySQL
    wait_for_mysql(container, Duration::from_secs(120))?;

    // State tracking
    let state = SqlState::new()?;

    // Load config using platform-appropriate storage (GCS for Google, S3 for AWS)
    let config = load_platform_config(project, platform, region, devdock_root.clone()).await?;
    let manager = SqlOperationsManager::new(project, platform, region, config);

    // Reset MySQL master
    output::info("Resetting MySQL master...");
    docker_mysql_exec(container, "RESET MASTER;")?;
    docker_mysql_exec(container, "SET GLOBAL max_allowed_packet=512*1024*1024;")?;

    // Copy and run schema scripts first (same as regular setup)
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

    // Download and import full instance dump
    output::info("Downloading full instance dump from GCS...");

    let export_dir = state.scripts_dir();
    std::fs::create_dir_all(&export_dir)?;

    let dump_path = manager
        .download_full_instance(&export_dir)
        .await
        .map_err(|e| {
            output::warn(&format!("Full instance download failed: {e}"));
            output::info(
                "To use table-by-table import instead, remove the --use-full-instance flag",
            );
            e
        })
        .context("Failed to download full instance dump")?;

    // Import the dump
    output::info(&format!("Importing full instance dump: {dump_path:?}"));
    manager.import_full_instance_dump(&dump_path)?;
    output::success("Full instance dump imported");

    // Overlay incremental tables with latest data (matches shell script lines 309-318)
    if !db_tables.is_empty() {
        output::info(&format!("⏩ Overlaying incremental tables with latest data: {db_tables}"));

        for entry in db_tables.split(';') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            output::info(&format!("Processing overlay: {entry}"));
            manager.export_from_cloud(entry, &export_dir, mode).await?;
        }

        // Import exported overlay dumps
        output::info("Importing overlay data...");
        let mut dump_files: Vec<_> = std::fs::read_dir(&export_dir)?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|e| e == "sql")
                    && !p.to_string_lossy().ends_with(".template")
                    && !p.to_string_lossy().contains("-full.sql") // Skip the full dump we already imported
            })
            .collect();
        dump_files.sort();

        for dump_file in &dump_files {
            output::info(&format!("⏩ Importing {:?}", dump_file.file_name().unwrap()));
            docker_mysql_import(container, dump_file)?;
            output::success(&format!("✅ Imported {:?}", dump_file.file_name().unwrap()));
        }
    }

    // Cleanup
    state.clear_scripts()?;

    output::success("✅ SQL setup with full instance complete");
    Ok(())
}

/// SQL cleanup
pub async fn sql_clean(project: &str, platform: &str, region: &str, db_tables: &str) -> Result<()> {
    output::info(&format!("Cleaning SQL for project {project}"));

    let devdock_root =
        std::env::var("DEVDOCK_ROOT").context("DEVDOCK_ROOT environment variable not set")?;
    let devdock_root = PathBuf::from(devdock_root);

    // Load merged config using platform-appropriate storage (GCS for Google, S3 for AWS)
    let config = load_platform_config(project, platform, region, devdock_root).await?;

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
