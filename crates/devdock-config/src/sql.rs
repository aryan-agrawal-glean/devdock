//! SQL instance utilities for DevDock.
//!
//! This module replaces python_scio SQL dependencies:
//! - python_scio.core.sql_constants (get_bastion_port_map)
//! - python_scio.deployment.sql (get_sql_instance_name_map_for_project, get_instance_id_for_db)
//! - python_scio.scripts.lib.utils.sql_util (check_if_sql_instances_have_public_IP)
//! - python_scio.scripts.lib.utils.bastion_util (is_bastion_available)

use crate::constants::{
    BACKEND_INSTANCE, FRONTEND_INSTANCE, IAM_PORT_START, PORT_START,
    SHARDED_SQL_INSTANCE_INFO_PREFIX,
};
use crate::ini_parser::ParsedIniConfig;
use anyhow::{Context, Result};
use std::collections::HashMap;
use tokio::process::Command;
use tracing::{debug, info, warn};

/// SQL instance information
#[derive(Debug, Clone)]
pub struct SqlInstanceInfo {
    /// Instance name (e.g., "frontendInstance")
    pub name: String,
    /// Full connection name (e.g., "project:region:instance-id")
    pub connection_name: String,
    /// Instance ID (last part of connection name)
    pub instance_id: String,
    /// Instance type (FE, BE, DOC)
    pub instance_type: &'static str,
}

/// Pre-fetched metadata for remote SQL configuration
#[derive(Debug, Clone)]
pub struct SqlRemoteMetadata {
    /// SQL instance map: instance_name -> connection_string
    pub instances: HashMap<String, String>,
    /// Whether instances have public IP enabled
    pub use_public_ip: bool,
    /// Whether bastion VM is available
    pub is_bastion_available: bool,
    /// Discovered zone of the `bastion` VM in this project, if one exists.
    ///
    /// Populated by `is_bastion_available` via `gcloud compute instances list`
    /// so callers (profile emitter, Tiltfile) use the *actual* zone rather
    /// than whatever the project's `primaryZone` says. In glean-apps-canary
    /// the bastion lives in `us-central1-c` even though primaryZone is
    /// `us-central1-a`; without this, `gcloud compute ssh` 404s.
    pub bastion_zone: Option<String>,
    /// Whether to use IAM authentication
    pub use_iam_auth: bool,
    /// Platform (google, aws, azure)
    pub platform: String,
    /// Project name
    pub project: String,
}

/// SQL utilities for DevDock
pub struct SqlUtils {
    project: String,
}

impl SqlUtils {
    /// Create a new SQL utilities instance
    pub fn new(project: &str) -> Self {
        Self { project: project.to_string() }
    }

    /// Get SQL instance name map from config
    ///
    /// Replaces: python_scio.deployment.sql.get_sql_instance_name_map_for_project
    pub fn get_instance_name_map(
        &self,
        config: &ParsedIniConfig,
        get_id_only: bool,
    ) -> HashMap<String, String> {
        let mut instance_map = HashMap::new();

        // Check for active instances in config
        if let Some(active_instances) = config.get("sql", "active.instances") {
            for instance_type in active_instances.split(',') {
                let instance_type = instance_type.trim();
                let key = format!("{instance_type}.googlecloud.instanceName");

                if let Some(full_name) = config.get("sql", &key) {
                    let value = if get_id_only {
                        // Extract instance ID from connection name
                        // Format: project:region:instance-id
                        full_name.split(':').next_back().unwrap_or(full_name).to_string()
                    } else {
                        full_name.clone()
                    };
                    instance_map.insert(instance_type.to_string(), value);
                }
            }
        } else {
            // Fallback: look for standard instance types
            for instance_type in &[FRONTEND_INSTANCE, BACKEND_INSTANCE] {
                let key = format!("{instance_type}.googlecloud.instanceName");
                if let Some(full_name) = config.get("sql", &key) {
                    let value = if get_id_only {
                        full_name.split(':').next_back().unwrap_or(full_name).to_string()
                    } else {
                        full_name.clone()
                    };
                    instance_map.insert((*instance_type).to_string(), value);
                }
            }
        }

        instance_map
    }

    /// Get instance ID for a specific database
    ///
    /// Replaces: python_scio.deployment.sql.get_instance_id_for_db
    pub fn get_instance_id_for_db(&self, config: &ParsedIniConfig, db_key: &str) -> Option<String> {
        // First get which instance type this DB uses
        let instance_key = format!("{db_key}.db.instance");
        let instance_type =
            config.get("sql", &instance_key).map_or(FRONTEND_INSTANCE, std::string::String::as_str);

        // Then get the instance connection name
        let connection_key = format!("{instance_type}.googlecloud.instanceName");
        config.get("sql", &connection_key).map(|full_name| {
            // Extract instance ID from connection name
            full_name.split(':').next_back().unwrap_or(full_name).to_string()
        })
    }

    /// Check if ALL SQL instances have public IP enabled
    ///
    /// Replaces: python_scio.scripts.lib.utils.sql_util.check_if_sql_instances_have_public_IP
    ///
    /// Returns true only when every instance has a public IP. If any instance
    /// lacks one, returns false so the caller falls back to bastion.
    ///
    /// Note: instances parameter contains full connection strings (project:region:instance-id)
    /// We extract the instance ID before calling gcloud.
    pub async fn check_public_ip(&self, instances: &HashMap<String, String>) -> Result<bool> {
        for (instance_type, connection_string) in instances {
            let instance_id = connection_string.split(':').next_back().unwrap_or(connection_string);

            debug!("Checking public IP for {} ({})", instance_type, instance_id);

            let output = Command::new("gcloud")
                .args([
                    "sql",
                    "instances",
                    "describe",
                    instance_id,
                    "--project",
                    &self.project,
                    "--format=value(settings.ipConfiguration.ipv4Enabled)",
                ])
                .output()
                .await
                .context("Failed to run gcloud sql instances describe")?;

            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.trim().to_lowercase() != "true" {
                    info!("Instance {} does not have public IP", instance_id);
                    return Ok(false);
                }
            } else {
                warn!("Failed to check public IP for {}, assuming no public IP", instance_id);
                return Ok(false);
            }
        }

        info!("All SQL instances have public IP enabled");
        Ok(true)
    }

    /// Check if a `bastion` VM exists in this project and return its zone.
    ///
    /// Replaces: python_scio.scripts.lib.utils.bastion_util.is_bastion_available
    ///
    /// We list instances by name across every zone (`gcloud compute
    /// instances list --filter=name=bastion`) instead of describing a
    /// hard-coded zone so we discover the real location. This matters
    /// for multi-projects like glean-apps-canary where the bastion
    /// lives in `us-central1-c` while `primaryZone` is `us-central1-a`.
    /// The discovered zone is propagated to the generated profile so the
    /// Tiltfile's `gcloud compute ssh` targets the right host.
    pub async fn is_bastion_available(&self) -> (bool, Option<String>) {
        let result = Command::new("gcloud")
            .args([
                "compute",
                "instances",
                "list",
                "--project",
                &self.project,
                "--filter",
                "name=bastion",
                "--format",
                "value(zone)",
            ])
            .output()
            .await;

        match result {
            Ok(output) if output.status.success() => {
                let zone = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if zone.is_empty() {
                    (false, None)
                } else {
                    // Multiple matches would be newline-separated; take the first.
                    let zone = zone.lines().next().unwrap_or("").trim().to_string();
                    debug!("Discovered bastion in {} (project {})", zone, self.project);
                    (true, Some(zone))
                }
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!("gcloud compute instances list failed for {}: {}", self.project, stderr);
                (false, None)
            }
            Err(e) => {
                warn!("Failed to check bastion availability: {}", e);
                (false, None)
            }
        }
    }

    /// Fetch all remote SQL metadata needed for configuration
    ///
    /// This replaces the Python __init__ logic in SqlServiceDependency
    pub async fn fetch_remote_metadata(
        project: &str,
        platform: &str,
        dynamic_ini: &ParsedIniConfig,
    ) -> Result<SqlRemoteMetadata> {
        let sql_utils = SqlUtils::new(project);

        // 1. Get instance map from dynamic.ini
        let instances = sql_utils.get_instance_name_map(dynamic_ini, false);

        // 2. Determine platform-specific behavior
        let (use_public_ip, is_bastion_available, bastion_zone) = match platform {
            "google" => {
                // For GCP, check actual infrastructure
                let use_public_ip = if !instances.is_empty() {
                    sql_utils.check_public_ip(&instances).await.unwrap_or(false)
                } else {
                    false
                };
                let (bastion_available, zone) = sql_utils.is_bastion_available().await;
                (use_public_ip, bastion_available, zone)
            }
            "aws" | "azure" => {
                // AWS/Azure always use their own connectivity patterns
                (false, false, None)
            }
            _ => (false, false, None),
        };

        // 3. Determine IAM auth setting
        let use_iam_auth = dynamic_ini
            .get("sql", "useDbIAMAuth")
            .and_then(|v| v.parse::<bool>().ok())
            .unwrap_or(true);

        // Special case: Public IP instances in test projects don't use IAM
        let use_iam_auth =
            if use_public_ip && !matches!(project, "glean-apps-canary" | "scio-engineering") {
                false
            } else {
                use_iam_auth
            };

        Ok(SqlRemoteMetadata {
            instances,
            use_public_ip,
            is_bastion_available,
            bastion_zone,
            use_iam_auth,
            platform: platform.to_string(),
            project: project.to_string(),
        })
    }

    /// Export SQL database to GCS
    ///
    /// Replaces: python_scio.scripts.lib.utils.sql_util.gcloud_export
    pub async fn export_to_gcs(
        &self,
        instance_id: &str,
        database: &str,
        gcs_uri: &str,
    ) -> Result<()> {
        info!("Exporting {} from {} to {}", database, instance_id, gcs_uri);

        let status = Command::new("gcloud")
            .args([
                "sql",
                "export",
                "sql",
                instance_id,
                gcs_uri,
                "--database",
                database,
                "--project",
                &self.project,
                "--async",
            ])
            .status()
            .await
            .context("Failed to run gcloud sql export")?;

        if !status.success() {
            anyhow::bail!("Failed to export database {database} from {instance_id}");
        }

        Ok(())
    }
}

/// Get bastion port map from config
///
/// Replaces: python_scio.core.sql_constants.get_bastion_port_map
///
/// The bastion port map is calculated by sorting instances and assigning sequential ports:
/// - Frontend instances come first (sorted by numeric suffix)
/// - Backend instances come next (sorted by numeric suffix)
/// - Document shards come last (sorted by shard number)
pub fn get_bastion_port_map(config: &ParsedIniConfig, use_iam_port: bool) -> HashMap<String, u16> {
    let mut port_map = HashMap::new();

    // Get active instances
    let active_instances = match config.get("sql", "active.instances") {
        Some(instances) => instances.split(',').map(|s| s.trim().to_string()).collect(),
        None => return port_map,
    };

    // Add document shards if configured
    let mut all_instances: Vec<String> = active_instances;
    let num_shards =
        config.get("sql", "document.numShards").and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);

    for shard in 0..num_shards {
        all_instances.push(format!("{SHARDED_SQL_INSTANCE_INFO_PREFIX}_{shard}"));
    }

    // Sort instances by type and numeric suffix
    all_instances.sort_by_key(|instance| get_sort_value(instance));

    // Assign ports
    let start_port = if use_iam_port { IAM_PORT_START } else { PORT_START };
    for (i, instance) in all_instances.iter().enumerate() {
        port_map.insert(instance.clone(), start_port + i as u16);
    }

    port_map
}

/// Get sort value for SQL instance ordering
///
/// Frontend instances: -1000 + index
/// Backend instances: 1000 + index
/// Document shards: 2000 + shard_number
fn get_sort_value(instance: &str) -> i32 {
    let base = if instance.starts_with(FRONTEND_INSTANCE) {
        -1000
    } else if instance.starts_with(BACKEND_INSTANCE) {
        1000
    } else if instance.starts_with(SHARDED_SQL_INSTANCE_INFO_PREFIX) {
        2000
    } else {
        0
    };

    base + extract_index(instance)
}

/// Extract numeric index from instance name
fn extract_index(instance: &str) -> i32 {
    let cleaned = instance
        .replace("backend", "")
        .replace("frontend", "")
        .replace("Instance", "")
        .replace("internal", "")
        .replace(&format!("{SHARDED_SQL_INSTANCE_INFO_PREFIX}_"), "");

    cleaned.parse().unwrap_or(0)
}

/// Databases to exclude from local import by default
pub const DEFAULT_EXCLUDED_DATABASES: &[&str] = &[
    "test",
    "jqe_user_identity",
    "internal_admin",
    "internal_permissions",
    "central_eval",
    "announcements",
    "user_generated_content",
];

/// Get list of databases to import for frontend instance
pub fn get_frontend_databases(config: &ParsedIniConfig) -> Vec<String> {
    config
        .get_sql_databases_by_instance(FRONTEND_INSTANCE)
        .into_iter()
        .filter(|db| !DEFAULT_EXCLUDED_DATABASES.contains(&db.as_str()))
        .collect()
}

/// Get list of databases to import for backend instance
pub fn get_backend_databases(config: &ParsedIniConfig) -> Vec<String> {
    config
        .get_sql_databases_by_instance(BACKEND_INSTANCE)
        .into_iter()
        .filter(|db| !DEFAULT_EXCLUDED_DATABASES.contains(&db.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CONFIG: &str = r"
[sql]
active.instances = frontendInstance,backendInstance
frontendInstance.googlecloud.instanceName = myproject:us-central1:frontend-sql-1
backendInstance.googlecloud.instanceName = myproject:us-central1:backend-sql-1
frontend_keyval.db.instance = frontendInstance
backend_data.db.instance = backendInstance
user_profile.db.instance = frontendInstance
";

    #[test]
    fn test_get_instance_name_map() {
        let config = ParsedIniConfig::parse_str(TEST_CONFIG).unwrap();
        let utils = SqlUtils::new("myproject");

        // Get full connection names
        let full_map = utils.get_instance_name_map(&config, false);
        assert_eq!(
            full_map.get("frontendInstance"),
            Some(&"myproject:us-central1:frontend-sql-1".to_string())
        );

        // Get instance IDs only
        let id_map = utils.get_instance_name_map(&config, true);
        assert_eq!(id_map.get("frontendInstance"), Some(&"frontend-sql-1".to_string()));
        assert_eq!(id_map.get("backendInstance"), Some(&"backend-sql-1".to_string()));
    }

    #[test]
    fn test_get_instance_id_for_db() {
        let config = ParsedIniConfig::parse_str(TEST_CONFIG).unwrap();
        let utils = SqlUtils::new("myproject");

        assert_eq!(
            utils.get_instance_id_for_db(&config, "frontend_keyval"),
            Some("frontend-sql-1".to_string())
        );
        assert_eq!(
            utils.get_instance_id_for_db(&config, "backend_data"),
            Some("backend-sql-1".to_string())
        );
    }

    #[test]
    fn test_get_bastion_port_map() {
        let config = ParsedIniConfig::parse_str(TEST_CONFIG).unwrap();

        // With IAM ports
        let iam_map = get_bastion_port_map(&config, true);
        assert_eq!(iam_map.get("frontendInstance"), Some(&13311));
        assert_eq!(iam_map.get("backendInstance"), Some(&13312));

        // Without IAM ports
        let non_iam_map = get_bastion_port_map(&config, false);
        assert_eq!(non_iam_map.get("frontendInstance"), Some(&3311));
        assert_eq!(non_iam_map.get("backendInstance"), Some(&3312));
    }

    #[test]
    fn test_get_sort_value() {
        // Frontend comes first
        assert!(get_sort_value("frontendInstance") < get_sort_value("backendInstance"));

        // Numbered instances are sorted
        assert!(get_sort_value("frontendInstance") < get_sort_value("frontendInstance2"));
        assert!(get_sort_value("backendInstance") < get_sort_value("backendInstance2"));

        // Document shards come last
        assert!(get_sort_value("backendInstance") < get_sort_value("documentDB_0"));
    }

    #[test]
    fn test_extract_index() {
        assert_eq!(extract_index("frontendInstance"), 0);
        assert_eq!(extract_index("frontendInstance2"), 2);
        assert_eq!(extract_index("backendInstance3"), 3);
        assert_eq!(extract_index("documentDB_5"), 5);
    }
}
