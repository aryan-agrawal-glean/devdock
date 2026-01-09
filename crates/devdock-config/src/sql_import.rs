//! SQL import/export operations for DevDock.
//!
//! This module replaces the Python SQL utilities:
//! - devdock/config/infra/sql/util/util.py
//! - devdock/config/infra/sql/util/sql_operations_manager.py
//! - devdock/config/infra/sql/util/google_sql_manager.py
//! - devdock/config/infra/sql/util/aws_sql_manager.py
//! - devdock/config/infra/sql/util/azure_sql_manager.py

use crate::gcs_storage::GcsStorage;
use crate::ini_parser::ParsedIniConfig;
use crate::sql::SqlUtils;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::process::Command as TokioCommand;
use tracing::{debug, info, warn};

/// SQL bucket URL pattern
const SQL_BUCKET_PATTERN: &str = "gs://sql-{project}/devdock";

/// SQL container name
const SQL_CONTAINER: &str = "devdock-mysql";
const SQL_CONTAINER_GAC: &str = "devdock-mysql-gac";

/// Platform-specific SQL manager trait
pub trait SqlPlatformManager: Send + Sync {
    /// Set up proxy connection (Cloud SQL Proxy, bastion tunnel, etc.)
    fn setup_proxy(&self, db_name: &str, port: u16) -> Result<Option<std::process::Child>>;

    /// Import data using a WHERE query
    fn import_data(
        &self,
        db_name: &str,
        tables: &str,
        query: &str,
        output_path: &Path,
        port: u16,
    ) -> Result<()>;

    /// Clean up proxy process
    fn cleanup_proxy(&self, process: std::process::Child);
}

/// Google Cloud SQL Manager
///
/// Authentication modes (matching Python implementation):
/// - Public IP instances: Uses `devdock_user` + password from Secret Manager
/// - Private IP instances (via bastion): Uses `compute-engine` user with NO password
pub struct GoogleSqlManager {
    project: String,
    region: String,
    config: ParsedIniConfig,
    sql_container: String,
    /// Track if we're using public IP or bastion (affects auth method)
    /// Uses AtomicBool for thread-safe interior mutability
    is_public_ip: std::sync::atomic::AtomicBool,
}

impl GoogleSqlManager {
    pub fn new(project: &str, region: &str, config: ParsedIniConfig) -> Self {
        let sql_container = if project == "glean-apps-canary" {
            SQL_CONTAINER_GAC
        } else {
            SQL_CONTAINER
        };

        Self {
            project: project.to_string(),
            region: region.to_string(),
            config,
            sql_container: sql_container.to_string(),
            is_public_ip: std::sync::atomic::AtomicBool::new(true), // Default, updated when setting up proxy
        }
    }

    /// Get instance ID for a database
    fn get_instance_id(&self, db_name: &str) -> Option<String> {
        let sql_utils = SqlUtils::new(&self.project);
        sql_utils.get_instance_id_for_db(&self.config, db_name)
    }

    /// Get password from Secret Manager (for public IP connections)
    fn get_sql_password(&self, secret_name: &str) -> Result<String> {
        println!(
            "     🔑 Fetching password from Secret Manager: {}",
            secret_name
        );
        let output = Command::new("gcloud")
            .args([
                "secrets",
                "versions",
                "access",
                "latest",
                &format!("--secret={}", secret_name),
            ])
            .output()
            .context("Failed to get secret")?;

        if !output.status.success() {
            anyhow::bail!(
                "Failed to get password from Secret Manager: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    /// Setup Cloud SQL Proxy (for public IP instances)
    fn setup_cloud_sql_proxy(&self, instance_id: &str, port: u16) -> Result<std::process::Child> {
        println!("     🔌 Starting Cloud SQL Proxy on port {}...", port);

        let connection_name = format!("{}:{}:{}", self.project, self.region, instance_id);
        println!("     📡 Connection: {}", connection_name);

        // Note: NOT using --auto-iam-authn here, we use devdock_user with password
        let child = Command::new("cloud-sql-proxy")
            .args([
                &connection_name,
                "-p",
                &port.to_string(),
                "--address",
                "0.0.0.0",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context(
                "Failed to start cloud-sql-proxy. Is it installed? brew install cloud-sql-proxy",
            )?;

        // Wait for proxy to be ready
        Self::wait_for_port(port, Duration::from_secs(30))?;

        println!("     ✅ Cloud SQL Proxy ready on port {}", port);
        Ok(child)
    }

    /// Setup bastion tunnel (for private instances)
    fn setup_bastion_proxy(&self, db_name: &str, local_port: u16) -> Result<std::process::Child> {
        println!("     🔌 Starting bastion tunnel on port {}...", local_port);

        // Get the SQL instance type for this DB
        let instance_type = self
            .config
            .get("sql", &format!("{}.db.instance", db_name))
            .cloned()
            .unwrap_or_else(|| "frontendInstance".to_string());

        // Map instance type to bastion port (matching Python)
        let remote_port = match instance_type.as_str() {
            "frontendInstance" => 13311,
            "backendInstance" => 13312,
            _ => 13311,
        };

        println!(
            "     📡 Tunneling localhost:{} -> bastion:127.0.0.1:{}",
            local_port, remote_port
        );

        let child = Command::new("gcloud")
            .args([
                "compute",
                "ssh",
                "bastion",
                "--project",
                &self.project,
                "--zone",
                "us-central1-a",
                "--tunnel-through-iap",
                "--",
                "-N",
                "-L",
                &format!("0.0.0.0:{}:127.0.0.1:{}", local_port, remote_port),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to start bastion tunnel")?;

        // Wait for tunnel to be ready
        Self::wait_for_port(local_port, Duration::from_secs(60))?;

        println!("     ✅ Bastion tunnel ready on port {}", local_port);
        Ok(child)
    }

    /// Wait for a port to become available
    fn wait_for_port(port: u16, timeout: Duration) -> Result<()> {
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            let output = Command::new("lsof")
                .args(["-i", &format!(":{}", port)])
                .output();

            if let Ok(out) = output {
                if out.status.success() && !out.stdout.is_empty() {
                    return Ok(());
                }
            }

            std::thread::sleep(Duration::from_secs(1));
        }

        anyhow::bail!("Timeout waiting for port {} to be ready", port)
    }

    /// Run mysqldump with appropriate auth based on connection type
    /// - Public IP: devdock_user + password from Secret Manager
    /// - Private IP (bastion): compute-engine user, NO password
    fn run_mysqldump(
        &self,
        db_name: &str,
        tables: &str,
        query: &str,
        output_path: &Path,
        port: u16,
    ) -> Result<()> {
        let mut cmd = Command::new("docker");
        cmd.args(["exec", &self.sql_container, "mysqldump"]);

        if self.is_public_ip.load(std::sync::atomic::Ordering::Relaxed) {
            // Public IP: use devdock_user with password
            let password = self.get_sql_password("sql-devdock_user-password")?;
            println!("     👤 Using user: devdock_user (with password from Secret Manager)");
            cmd.args(["-u", "devdock_user", &format!("--password={}", password)]);
        } else {
            // Bastion: use compute-engine user, NO password
            println!("     👤 Using user: compute-engine (no password via bastion)");
            cmd.args(["-u", "compute-engine"]);
        }

        cmd.args([
            "--single-transaction",
            "--lock-tables=false",
            "--no-tablespaces", // Avoid RELOAD privilege requirement
            "--skip-add-locks",
            "-h",
            "host.docker.internal",
            "-P",
            &port.to_string(),
        ]);

        if !query.is_empty() {
            cmd.arg(format!("--where={}", query));
        }

        cmd.arg(db_name);

        if !tables.is_empty() {
            // Split tables by comma and add each
            for table in tables.split(',') {
                let t = table.trim();
                if !t.is_empty() {
                    cmd.arg(t);
                }
            }
        }

        println!("     📤 Running mysqldump for {}...", db_name);
        println!("     ⏳ Executing mysqldump (this may take a while)...");

        let output = cmd.output().context("Failed to run mysqldump")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if self.is_public_ip.load(std::sync::atomic::Ordering::Relaxed) {
                anyhow::bail!(
                    "mysqldump failed:\n{}\n\nTroubleshooting (public IP mode):\n\
                     1. Check Secret Manager secret 'sql-devdock_user-password' exists\n\
                     2. Ensure devdock_user has SELECT permissions on {}\n\
                     3. Verify cloud-sql-proxy is running",
                    stderr,
                    db_name
                );
            } else {
                anyhow::bail!(
                    "mysqldump failed:\n{}\n\nTroubleshooting (bastion mode):\n\
                     1. Ensure 'compute-engine' MySQL user exists on the instance\n\
                     2. Ensure compute-engine has SELECT permissions on {}\n\
                     3. Verify bastion tunnel is connected",
                    stderr,
                    db_name
                );
            }
        }

        std::fs::write(output_path, &output.stdout).context("Failed to write SQL dump")?;

        let size = output.stdout.len();
        println!(
            "     ✅ Exported {} ({} bytes) to {:?}",
            db_name, size, output_path
        );
        Ok(())
    }
}

impl SqlPlatformManager for GoogleSqlManager {
    fn setup_proxy(&self, db_name: &str, port: u16) -> Result<Option<std::process::Child>> {
        let instance_id = self
            .get_instance_id(db_name)
            .ok_or_else(|| anyhow::anyhow!("Could not find instance for database: {}", db_name))?;

        println!("     🔍 Found SQL instance: {}", instance_id);

        // Check if instance has public IP
        let has_public = std::thread::spawn({
            let project = self.project.clone();
            let instance = instance_id.clone();
            move || {
                tokio::runtime::Runtime::new().unwrap().block_on(async {
                    let output = TokioCommand::new("gcloud")
                        .args([
                            "sql",
                            "instances",
                            "describe",
                            &instance,
                            "--project",
                            &project,
                            "--format=value(settings.ipConfiguration.ipv4Enabled)",
                        ])
                        .output()
                        .await;

                    match output {
                        Ok(out) if out.status.success() => {
                            String::from_utf8_lossy(&out.stdout).trim().to_lowercase() == "true"
                        }
                        _ => false,
                    }
                })
            }
        })
        .join()
        .unwrap_or(false);

        // Store the public IP status for import_data to use
        self.is_public_ip
            .store(has_public, std::sync::atomic::Ordering::Relaxed);

        if has_public {
            println!("     🌐 Instance has public IP - using Cloud SQL Proxy + devdock_user");
            Ok(Some(self.setup_cloud_sql_proxy(&instance_id, port)?))
        } else {
            println!("     🔒 Instance is private - using bastion tunnel + compute-engine user");
            Ok(Some(self.setup_bastion_proxy(db_name, port)?))
        }
    }

    fn import_data(
        &self,
        db_name: &str,
        tables: &str,
        query: &str,
        output_path: &Path,
        port: u16,
    ) -> Result<()> {
        println!("     📊 Exporting {} from Google Cloud SQL...", db_name);
        self.run_mysqldump(db_name, tables, query, output_path, port)
    }

    fn cleanup_proxy(&self, mut process: std::process::Child) {
        println!("     🧹 Cleaning up proxy process...");
        let _ = process.kill();
        let _ = process.wait();
    }
}

/// AWS RDS Manager
pub struct AwsSqlManager {
    #[allow(dead_code)]
    project: String,
    region: String,
}

impl AwsSqlManager {
    pub fn new(project: &str, region: &str) -> Self {
        Self {
            project: project.to_string(),
            region: region.to_string(),
        }
    }

    /// Get bastion instance ID
    fn get_bastion_instance_id(&self) -> Result<String> {
        let output = Command::new("aws")
            .args([
                "ec2",
                "describe-instances",
                "--region",
                &self.region,
                "--filters",
                "Name=tag:Name,Values=bastion",
                "--query",
                "Reservations[0].Instances[0].InstanceId",
                "--output",
                "text",
            ])
            .output()
            .context("Failed to get bastion instance")?;

        if !output.status.success() {
            anyhow::bail!(
                "Failed to get bastion: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    /// Get SQL endpoint from config
    pub fn get_sql_endpoint(&self, config: &ParsedIniConfig, db_name: &str) -> Option<String> {
        let instance_type = config.get("sql", &format!("{}.db.instance", db_name))?;
        config
            .get("sql", &format!("{}.aws.instanceEndpoint", instance_type))
            .cloned()
    }
}

impl SqlPlatformManager for AwsSqlManager {
    fn setup_proxy(&self, _db_name: &str, _port: u16) -> Result<Option<std::process::Child>> {
        let bastion_id = self.get_bastion_instance_id()?;

        info!("Starting AWS SSM session to bastion {}...", bastion_id);

        let payload = serde_json::json!({
            "portNumber": ["3306"],
            "localPortNumber": ["3309"],
            "host": ["rds-endpoint"]  // Would need actual endpoint
        });

        let child = Command::new("aws")
            .args([
                "ssm",
                "start-session",
                "--target",
                &bastion_id,
                "--document-name",
                "AWS-StartPortForwardingSessionToRemoteHost",
                "--parameters",
                &payload.to_string(),
                "--region",
                &self.region,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to start SSM session")?;

        // Wait for port
        GoogleSqlManager::wait_for_port(3309, Duration::from_secs(60))?;

        Ok(Some(child))
    }

    fn import_data(
        &self,
        db_name: &str,
        tables: &str,
        query: &str,
        output_path: &Path,
        _port: u16,
    ) -> Result<()> {
        info!("Importing {} from AWS RDS...", db_name);

        // Generate IAM auth token
        let token_output = Command::new("aws")
            .args([
                "rds",
                "generate-db-auth-token",
                "--hostname",
                "rds-endpoint", // Would need actual endpoint
                "--port",
                "3306",
                "--region",
                &self.region,
                "--username",
                "glean",
            ])
            .output()
            .context("Failed to generate auth token")?;

        let password = String::from_utf8(token_output.stdout)?.trim().to_string();

        let mut cmd = Command::new("docker");
        cmd.args([
            "exec",
            SQL_CONTAINER,
            "mysqldump",
            "-u",
            "glean",
            &format!("--password={}", password),
            "-h",
            "host.docker.internal",
            "-P",
            "3309",
            "--enable-cleartext-plugin",
            "--lock-tables=false",
            "--set-gtid-purged=OFF",
            db_name,
        ]);

        if !tables.is_empty() {
            for table in tables.split(',') {
                cmd.arg(table.trim());
            }
            if !query.is_empty() {
                cmd.arg(format!("--where={}", query));
            }
        }

        let output = cmd.output().context("Failed to run mysqldump")?;

        if !output.status.success() {
            anyhow::bail!(
                "mysqldump failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        std::fs::write(output_path, &output.stdout)?;
        Ok(())
    }

    fn cleanup_proxy(&self, mut process: std::process::Child) {
        let _ = process.kill();
        let _ = process.wait();
    }
}

/// SQL Operations Manager - main entry point
pub struct SqlOperationsManager {
    project: String,
    platform: String,
    #[allow(dead_code)]
    region: String,
    config: ParsedIniConfig,
    sql_container: String,
}

impl SqlOperationsManager {
    pub fn new(project: &str, platform: &str, region: &str, config: ParsedIniConfig) -> Self {
        let sql_container = if project == "glean-apps-canary" {
            SQL_CONTAINER_GAC
        } else {
            SQL_CONTAINER
        };

        Self {
            project: project.to_string(),
            platform: platform.to_string(),
            region: region.to_string(),
            config,
            sql_container: sql_container.to_string(),
        }
    }

    /// Parse db:tables:query format
    pub fn parse_db_tables_query(input: &str) -> (String, String, Option<String>) {
        let parts: Vec<&str> = input.split(':').collect();

        let db = parts.first().map(|s| s.to_string()).unwrap_or_default();
        let tables = parts.get(1).map(|s| s.to_string()).unwrap_or_default();
        let query = if parts.len() >= 3 {
            Some(parts[2..].join(":"))
        } else {
            None
        };

        (db, tables, query)
    }

    /// Clean up SQL databases/tables
    pub fn clean_sql(&self, db_tables: &str) -> Result<()> {
        if db_tables.is_empty() {
            // Get all databases and clean non-system ones
            let output = Command::new("docker")
                .args([
                    "exec",
                    &self.sql_container,
                    "mysql",
                    "-u",
                    "root",
                    "-e",
                    "SHOW DATABASES;",
                ])
                .output()?;

            let databases = String::from_utf8_lossy(&output.stdout);
            let system_dbs = [
                "information_schema",
                "mysql",
                "performance_schema",
                "sys",
                "Database",
            ];

            let to_drop: Vec<&str> = databases
                .lines()
                .filter(|db| !db.is_empty() && !system_dbs.contains(db))
                .collect();

            for db in to_drop {
                info!("Dropping database: {}", db);
                Command::new("docker")
                    .args([
                        "exec",
                        &self.sql_container,
                        "mysql",
                        "-u",
                        "root",
                        "-e",
                        &format!("DROP DATABASE IF EXISTS {};", db),
                    ])
                    .status()?;
            }
        } else {
            for entry in db_tables.split(';') {
                let (db, tables, _) = Self::parse_db_tables_query(entry);
                if db.is_empty() {
                    continue;
                }

                if tables.is_empty() {
                    info!("Dropping database: {}", db);
                    Command::new("docker")
                        .args([
                            "exec",
                            &self.sql_container,
                            "mysql",
                            "-u",
                            "root",
                            "-e",
                            &format!("DROP DATABASE IF EXISTS {};", db),
                        ])
                        .status()?;
                } else {
                    info!("Dropping tables: {} from {}", tables, db);
                    Command::new("docker")
                        .args([
                            "exec",
                            &self.sql_container,
                            "mysql",
                            "-u",
                            "root",
                            "-e",
                            &format!("USE {}; DROP TABLE IF EXISTS {};", db, tables),
                        ])
                        .status()?;
                }
            }
        }

        info!("✅ Cleaned up SQL");
        Ok(())
    }

    /// Fast export from GCS bucket (pre-exported dumps)
    pub async fn fast_export(&self, db: &str, target_dir: &Path) -> Result<()> {
        let storage = GcsStorage::with_default_cache(&self.project)?;
        let gcs_path = format!(
            "{}/{}.sql",
            SQL_BUCKET_PATTERN.replace("{project}", &self.project),
            db
        );
        let target_path = target_dir.join(format!("{}.sql", db));

        println!("     📥 Downloading: {}", gcs_path);
        println!("     📁 Target: {:?}", target_path);

        storage
            .fetch_to_path(&gcs_path, &target_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to download from GCS. Check:\n\
                 1. GCS bucket exists: gs://sql-{}/devdock/\n\
                 2. File exists: {}.sql\n\
                 3. You have read permissions",
                    self.project, db
                )
            })?;

        Ok(())
    }

    /// Fast export multiple DBs in parallel
    pub async fn fast_export_parallel(&self, dbs: &[String], target_dir: &Path) -> Result<()> {
        let futures: Vec<_> = dbs
            .iter()
            .map(|db| self.fast_export(db, target_dir))
            .collect();

        futures::future::try_join_all(futures).await?;
        Ok(())
    }

    /// Export databases from cloud
    ///
    /// In "fast" mode: Downloads pre-exported SQL dumps from GCS bucket
    /// In "latest" mode: Uses Cloud SQL Admin API (for GCP) or proxy (for AWS/Azure)
    ///
    /// NOTE: For GCP, "latest" mode requires the project to have export permissions
    /// and uses gcloud sql export, NOT direct mysqldump (which fails with IAM auth).
    pub async fn export_from_cloud(
        &self,
        db_tables_query: &str,
        target_dir: &Path,
        mode: &str,
    ) -> Result<()> {
        if db_tables_query.is_empty() {
            return Ok(());
        }

        let (db, tables, query) = Self::parse_db_tables_query(db_tables_query);

        // Debug: show what we're checking
        println!("  📊 Export check for '{}':", db_tables_query);
        println!(
            "     - db: '{}', tables: '{}', query: {:?}",
            db, tables, query
        );
        println!(
            "     - mode: '{}', platform: '{}', project: '{}'",
            mode, self.platform, self.project
        );

        // Check if fast export is available
        // Python behavior: fast export is used even when tables is specified,
        // because fast export downloads the entire DB dump anyway
        let fast_export_config = self.get_fast_export_config();

        let project_ok = fast_export_config.projects.contains(&self.project);
        let db_ok = fast_export_config.dbs.contains(&db);
        let mode_ok = mode == "fast";
        let query_ok = query.is_none();

        println!("     - fast export check: project_in_list={}, db_in_list={}, mode_is_fast={}, no_query={}",
                 project_ok, db_ok, mode_ok, query_ok);

        let can_use_fast = mode_ok && project_ok && db_ok && query_ok;

        if can_use_fast {
            println!("  ⚡ FAST EXPORT: {} (downloading from GCS bucket)", db);
            self.fast_export(&db, target_dir).await?;
            println!("  ✅ Fast export complete for {}", db);
        } else if let Some(ref where_clause) = query {
            // Query-based export requires proxy connection + mysqldump
            // This matches Python's behavior: setup proxy, clean local data, mysqldump with --where
            println!(
                "  🔍 QUERY-BASED EXPORT: {} (via mysqldump with --where)",
                db
            );
            println!("     WHERE clause: {}", where_clause);

            self.export_via_proxy_mysqldump(&db, &tables, where_clause, target_dir)?;
            println!("  ✅ Query-based export complete for {}", db);
        } else {
            // For "latest" mode or non-fast-export DBs
            // Use gcloud sql export (Cloud SQL Admin API) - same as Python's export_databases_from_cloud
            println!("  🐢 SLOW EXPORT: {} (via gcloud sql export)", db);
            println!(
                "     Reason: mode_is_fast={}, project_in_list={}, db_in_list={}",
                mode_ok, project_ok, db_ok
            );

            self.export_via_gcloud_sql_export(&db, &tables, target_dir)
                .await?;
            println!("  ✅ Slow export complete for {}", db);
        }

        // Modify SQL dump to add CREATE DATABASE and USE statements
        Self::modify_sql_dump(&db, target_dir)?;

        Ok(())
    }

    /// Export using gcloud sql export (Cloud SQL Admin API)
    /// This matches Python's export_databases_from_cloud() function:
    /// 1. Export to temp GCS bucket using gcloud sql export sql
    /// 2. Copy from GCS to local
    /// 3. Clean up temp GCS directory
    async fn export_via_gcloud_sql_export(
        &self,
        db: &str,
        tables: &str,
        target_dir: &Path,
    ) -> Result<()> {
        // Get instance ID for this database
        let sql_utils = SqlUtils::new(&self.project);
        let instance_id = sql_utils
            .get_instance_id_for_db(&self.config, db)
            .ok_or_else(|| anyhow::anyhow!("Could not find SQL instance for database: {}", db))?;

        println!("     🔍 Found SQL instance: {}", instance_id);

        // Create temp GCS directory (matching Python: gs://sql-{project}/tmp-{account}-{timestamp})
        let timestamp = chrono::Utc::now().format("%H:%M:%S:%f").to_string();
        let account = self.get_gcloud_account().await?;
        let tmp_gcs_dir = format!("gs://sql-{}/tmp-{}-{}", self.project, account, timestamp);
        let gcs_target = format!("{}/{}.sql", tmp_gcs_dir, db);

        println!("     📤 Exporting to GCS: {}", gcs_target);

        // Build gcloud sql export command
        let mut cmd_args = vec![
            "sql".to_string(),
            "export".to_string(),
            "sql".to_string(),
            instance_id.clone(),
            gcs_target.clone(),
            format!("--project={}", self.project),
            format!("--database={}", db),
        ];

        // Add table filter if specified
        if !tables.is_empty() {
            cmd_args.push(format!("--table={}", tables));
        }

        println!("     ⏳ Running: gcloud {}", cmd_args.join(" "));

        let output = TokioCommand::new("gcloud")
            .args(&cmd_args)
            .output()
            .await
            .context("Failed to run gcloud sql export")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Clean up temp directory on failure
            let _ = self.delete_gcs_temp(&tmp_gcs_dir).await;
            anyhow::bail!(
                "gcloud sql export failed:\n{}\n\nNote: This requires the Cloud SQL instance to have write access to gs://sql-{}/ bucket",
                stderr, self.project
            );
        }

        println!("     ✅ Export to GCS complete");

        // Copy from GCS to local
        let local_path = target_dir.join(format!("{}.sql", db));
        println!("     📥 Copying from GCS to {:?}", local_path);

        let cp_output = TokioCommand::new("gcloud")
            .args(["storage", "cp", &gcs_target, local_path.to_str().unwrap()])
            .output()
            .await
            .context("Failed to copy from GCS")?;

        if !cp_output.status.success() {
            let stderr = String::from_utf8_lossy(&cp_output.stderr);
            // Clean up temp directory on failure
            let _ = self.delete_gcs_temp(&tmp_gcs_dir).await;
            anyhow::bail!("Failed to copy from GCS: {}", stderr);
        }

        // Clean up temp GCS directory
        println!("     🧹 Cleaning up temp GCS directory");
        let _ = self.delete_gcs_temp(&tmp_gcs_dir).await;

        // Verify file exists and has content
        let metadata = std::fs::metadata(&local_path)?;
        println!("     ✅ Downloaded {} ({} bytes)", db, metadata.len());

        Ok(())
    }

    /// Get current gcloud account
    async fn get_gcloud_account(&self) -> Result<String> {
        let output = TokioCommand::new("gcloud")
            .args(["config", "get-value", "account"])
            .output()
            .await
            .context("Failed to get gcloud account")?;

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    /// Delete temp GCS directory
    async fn delete_gcs_temp(&self, gcs_dir: &str) -> Result<()> {
        let _ = TokioCommand::new("gcloud")
            .args(["storage", "rm", "-r", gcs_dir])
            .output()
            .await;
        Ok(())
    }

    /// Export using proxy + mysqldump with WHERE clause
    /// This matches Python's query-based export flow:
    /// 1. Setup proxy (Cloud SQL Proxy for public IP, bastion for private)
    /// 2. Clean local data using WHERE clause
    /// 3. mysqldump with --where clause
    /// 4. Cleanup proxy
    fn export_via_proxy_mysqldump(
        &self,
        db: &str,
        tables: &str,
        where_clause: &str,
        target_dir: &Path,
    ) -> Result<()> {
        // Port for proxy connection
        let port: u16 = 3308;

        // Create platform-specific manager based on platform
        let manager: Box<dyn SqlPlatformManager> = match self.platform.as_str() {
            "google" => Box::new(GoogleSqlManager::new(
                &self.project,
                "us-central1",
                self.config.clone(),
            )),
            "aws" => Box::new(AwsSqlManager::new(&self.project, "us-west-2")),
            "azure" => {
                // Azure query-based export not yet implemented in Rust
                anyhow::bail!(
                    "Query-based export for Azure is not yet implemented.\n\
                     Use Python devdock for Azure query-based exports."
                );
            }
            _ => anyhow::bail!("Unsupported platform: {}", self.platform),
        };

        // Setup proxy connection
        println!("     🔌 Setting up proxy connection...");
        let proxy_process = manager.setup_proxy(db, port)?;

        if proxy_process.is_none() {
            anyhow::bail!("Failed to setup proxy connection");
        }
        let proxy_process = proxy_process.unwrap();

        // Use a closure to ensure cleanup happens
        let result = (|| -> Result<()> {
            // Clean local data using WHERE clause first
            println!("     🧹 Cleaning local data matching WHERE clause...");
            self.clean_devdock_sql_using_where_clause(db, tables, where_clause)?;

            // Run mysqldump with WHERE clause
            let output_path = target_dir.join(format!("{}.sql", db));
            manager.import_data(db, tables, where_clause, &output_path, port)?;

            Ok(())
        })();

        // Always cleanup proxy
        manager.cleanup_proxy(proxy_process);

        result
    }

    /// Clean local SQL data using a WHERE clause
    /// Matches Python's clean_devdock_sql_using_where_clause
    fn clean_devdock_sql_using_where_clause(
        &self,
        database: &str,
        table: &str,
        where_clause: &str,
    ) -> Result<()> {
        println!(
            "     Dropping entries from {}.{} WHERE {}",
            database, table, where_clause
        );

        let delete_sql = format!("DELETE FROM {}.{} WHERE {}", database, table, where_clause);

        let output = Command::new("docker")
            .args([
                "exec",
                &self.sql_container,
                "mysql",
                "-u",
                "root",
                "-e",
                &delete_sql,
            ])
            .output()
            .context("Failed to clean local SQL data")?;

        if !output.status.success() {
            // Log warning but don't fail - table might not exist yet
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!("     ⚠️  Warning cleaning local data: {}", stderr.trim());
        } else {
            println!("     ✅ Cleaned local data");
        }

        Ok(())
    }

    /// Export using Cloud SQL Admin API (gcloud sql export)
    /// This exports to a GCS bucket and then downloads locally
    /// NOTE: Currently unused - kept for reference
    #[allow(dead_code)]
    async fn export_via_cloud_sql_api_old(
        &self,
        db: &str,
        tables: &str,
        target_dir: &Path,
    ) -> Result<()> {
        // First, clean up existing entries
        self.clean_sql(&format!("{}:{}", db, tables))?;

        // Get the SQL instance ID
        let sql_utils = SqlUtils::new(&self.project);
        let instance_id = sql_utils
            .get_instance_id_for_db(&self.config, db)
            .ok_or_else(|| anyhow::anyhow!("Could not find SQL instance for database: {}", db))?;

        // Export to GCS bucket
        let bucket_uri = format!("gs://sql-{}/devdock-export/{}.sql", self.project, db);

        info!("Exporting {} to {}...", db, bucket_uri);

        let mut cmd_args = vec![
            "sql".to_string(),
            "export".to_string(),
            "sql".to_string(),
            instance_id.clone(),
            bucket_uri.clone(),
            format!("--database={}", db),
            format!("--project={}", self.project),
            "--async".to_string(),
        ];

        // Add table filter if specified
        if !tables.is_empty() {
            for table in tables.split(',') {
                cmd_args.push(format!("--table={}", table.trim()));
            }
        }

        let output = TokioCommand::new("gcloud")
            .args(&cmd_args)
            .output()
            .await
            .context("Failed to run gcloud sql export")?;

        if !output.status.success() {
            anyhow::bail!(
                "gcloud sql export failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Wait for export to complete (poll operation status)
        info!("Waiting for export to complete...");
        tokio::time::sleep(Duration::from_secs(30)).await;

        // Download from GCS
        let target_path = target_dir.join(format!("{}.sql", db));
        let storage = GcsStorage::with_default_cache(&self.project)?;
        storage.fetch_to_path(&bucket_uri, &target_path).await?;

        info!("Downloaded {} to {:?}", db, target_path);
        Ok(())
    }

    /// Export via Cloud SQL proxy/bastion with IAM authentication
    /// NOTE: Currently unused - we use gcloud sql export instead
    #[allow(dead_code)]
    async fn export_via_proxy(
        &self,
        db: &str,
        tables: &str,
        query: Option<&str>,
        target_dir: &Path,
    ) -> Result<()> {
        match self.platform.as_str() {
            "google" => {
                let manager =
                    GoogleSqlManager::new(&self.project, &self.region, self.config.clone());
                let proxy = manager.setup_proxy(db, 3308)?;

                let result = manager.import_data(
                    db,
                    tables,
                    query.unwrap_or(""),
                    &target_dir.join(format!("{}.sql", db)),
                    3308,
                );

                if let Some(p) = proxy {
                    manager.cleanup_proxy(p);
                }

                result
            }
            "aws" => {
                let manager = AwsSqlManager::new(&self.project, &self.region);
                let proxy = manager.setup_proxy(db, 3309)?;

                let result = manager.import_data(
                    db,
                    tables,
                    query.unwrap_or(""),
                    &target_dir.join(format!("{}.sql", db)),
                    3309,
                );

                if let Some(p) = proxy {
                    manager.cleanup_proxy(p);
                }

                result
            }
            _ => anyhow::bail!("Unsupported platform: {}", self.platform),
        }
    }

    /// Modify SQL dump to add CREATE DATABASE and USE statements
    pub fn modify_sql_dump(db_name: &str, directory: &Path) -> Result<()> {
        let file_path = directory.join(format!("{}.sql", db_name));

        if !file_path.exists() {
            warn!("{}.sql not found in {:?}", db_name, directory);
            return Ok(());
        }

        let content = std::fs::read(&file_path)?;

        // Check if already modified
        if content.starts_with(b"CREATE") {
            debug!("Skipped {}.sql: already modified", db_name);
            return Ok(());
        }

        let prefix = format!(
            "CREATE DATABASE IF NOT EXISTS {};\nUSE {};\n\n",
            db_name, db_name
        );

        let mut new_content = prefix.into_bytes();
        new_content.extend_from_slice(&content);

        std::fs::write(&file_path, new_content)?;
        info!(
            "Modified {}.sql with CREATE DATABASE and USE statements",
            db_name
        );

        Ok(())
    }

    /// Get fast export configuration
    /// Loaded from devdock/config/infra/sql/util/fast_export_dbs.yaml
    fn get_fast_export_config(&self) -> FastExportConfig {
        // Try to load from YAML file
        if let Ok(devdock_root) = std::env::var("DEVDOCK_ROOT") {
            let yaml_path =
                Path::new(&devdock_root).join("devdock/config/infra/sql/util/fast_export_dbs.yaml");

            if let Ok(content) = std::fs::read_to_string(&yaml_path) {
                if let Ok(config) = FastExportConfig::from_yaml(&content) {
                    println!("     📄 Loaded fast export config from {:?}", yaml_path);
                    return config;
                }
            }
        }

        // Fallback: try relative to current working directory
        let fallback_paths = [
            "devdock/config/infra/sql/util/fast_export_dbs.yaml",
            "../devdock/config/infra/sql/util/fast_export_dbs.yaml",
        ];

        for path in fallback_paths {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(config) = FastExportConfig::from_yaml(&content) {
                    println!("     📄 Loaded fast export config from {}", path);
                    return config;
                }
            }
        }

        // Final fallback: empty config (will trigger slow export for everything)
        println!("     ⚠️  Could not load fast_export_dbs.yaml - no fast exports available");
        FastExportConfig::default()
    }
}

/// Fast export configuration
/// Loaded from devdock/config/infra/sql/util/fast_export_dbs.yaml
#[derive(Default)]
struct FastExportConfig {
    projects: HashSet<String>,
    dbs: HashSet<String>,
}

impl FastExportConfig {
    /// Parse from YAML content
    fn from_yaml(content: &str) -> Result<Self> {
        #[derive(serde::Deserialize)]
        struct YamlConfig {
            projects: Vec<String>,
            dbs: Vec<String>,
        }

        let yaml: YamlConfig =
            serde_yaml::from_str(content).context("Failed to parse fast_export_dbs.yaml")?;

        Ok(Self {
            projects: yaml.projects.into_iter().collect(),
            dbs: yaml.dbs.into_iter().collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_db_tables_query() {
        let (db, tables, query) = SqlOperationsManager::parse_db_tables_query("mydb");
        assert_eq!(db, "mydb");
        assert_eq!(tables, "");
        assert!(query.is_none());

        let (db, tables, query) = SqlOperationsManager::parse_db_tables_query("mydb:table1,table2");
        assert_eq!(db, "mydb");
        assert_eq!(tables, "table1,table2");
        assert!(query.is_none());

        let (db, tables, query) =
            SqlOperationsManager::parse_db_tables_query("mydb:table1:id > 100");
        assert_eq!(db, "mydb");
        assert_eq!(tables, "table1");
        assert_eq!(query, Some("id > 100".to_string()));
    }
}
