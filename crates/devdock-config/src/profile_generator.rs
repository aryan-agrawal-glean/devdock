//! Temporary profile generator for Tilt.

use crate::dependency_graph::{ComponentType, DependencyGraph};
use crate::ini_parser::ParsedIniConfig;
use crate::opensearch::get_elastic_namespace;
use crate::platform::{Platform, TempProfile};
use anyhow::Result;
use devdock_core::profile::Profile;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use tracing::warn;

/// Generator for temp_profile.yaml
pub struct TempProfileGenerator {
    profile: TempProfileData,
    /// Reference to dynamic.ini for reading production config values
    dynamic_ini: Option<ParsedIniConfig>,
    /// Pre-fetched remote SQL metadata (for remote SQL configuration)
    sql_remote_metadata: Option<crate::sql::SqlRemoteMetadata>,
}

/// Helper function to skip empty strings (but not for zone field)
fn is_empty_string(_s: &str) -> bool {
    // Never skip - Python always outputs zone even if empty
    false
}

/// Helper function to skip "disabled" dns_kubectl
fn is_disabled_or_none(opt: &Option<String>) -> bool {
    match opt {
        None => true,
        Some(s) => s == "disabled",
    }
}

#[derive(Debug, Serialize)]
pub struct TempProfileData {
    pub project: String,
    pub platform: String,
    pub devdock_root: String,
    pub region: String,
    #[serde(skip_serializing_if = "is_empty_string")]
    pub zone: String,
    pub architecture: String,
    #[serde(skip_serializing_if = "is_empty_string")]
    pub project_number: String,
    pub enable_service_metrics: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_profile: Option<String>,
    #[serde(skip_serializing_if = "is_disabled_or_none")]
    pub dns_kubectl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub k8s_service_account: Option<String>,
    pub debug_mode: String,
    pub vault_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql_account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_account_id: Option<String>,
    pub services: HashMap<String, ServiceProfileEntry>,
    pub infra: HashMap<String, InfraProfileEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipelines: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_suite: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    /// Multi-project configuration (e.g., glean-apps-canary)
    #[serde(rename = "glean_apps_canary", skip_serializing_if = "Option::is_none")]
    pub multi_project: Option<MultiProjectProfile>,
}

/// Nested configuration for multi-project (glean-apps-canary)
#[derive(Debug, Clone, Serialize)]
pub struct MultiProjectProfile {
    pub project: String,
    pub platform: String,
    pub region: String,
    pub zone: String,
    pub project_number: String,
    pub infra: HashMap<String, InfraProfileEntry>,
    pub services: HashMap<String, ServiceProfileEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceProfileEntry {
    #[serde(rename = "type")]
    pub service_type: String,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hot_reload: Option<String>,

    // Hook-specific fields (for pre_setup and post_setup)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends_on_services: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,

    // Allow any additional fields from the profile to pass through
    #[serde(flatten, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InfraProfileEntry {
    #[serde(rename = "type")]
    pub infra_type: String,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Get the OS name matching Python's platform.system().lower()
fn get_os_name() -> String {
    if cfg!(target_os = "macos") {
        "darwin".to_string()
    } else if cfg!(target_os = "linux") {
        "linux".to_string()
    } else if cfg!(target_os = "windows") {
        "windows".to_string()
    } else {
        std::env::consts::OS.to_string()
    }
}

impl TempProfileGenerator {
    pub fn new(workspace_root: &Path, profile_path: Option<&str>) -> Self {
        // Python uses platform.system().lower() which returns "darwin" on macOS
        let architecture = get_os_name();

        let profile = TempProfileData {
            project: String::new(),
            platform: String::new(),
            devdock_root: workspace_root.display().to_string(),
            region: String::new(),
            zone: String::new(),
            architecture,
            project_number: String::new(),
            enable_service_metrics: "True".to_string(),
            current_profile: profile_path.map(std::string::ToString::to_string),
            dns_kubectl: Some("enabled".to_string()), // Default to enabled (matches Python)
            k8s_service_account: None,
            debug_mode: "False".to_string(),
            vault_key: String::new(),
            sql_account: None,
            aws_account_id: None,
            services: HashMap::new(),
            infra: HashMap::new(),
            pipelines: None,
            test_suite: None,
            env: None,
            multi_project: None,
        };

        Self { profile, dynamic_ini: None, sql_remote_metadata: None }
    }

    /// Set the dynamic.ini config for reading production values
    pub fn with_dynamic_ini(mut self, dynamic_ini: ParsedIniConfig) -> Self {
        self.dynamic_ini = Some(dynamic_ini);
        self
    }

    /// Set pre-fetched SQL remote metadata
    pub fn with_sql_metadata(mut self, metadata: crate::sql::SqlRemoteMetadata) -> Self {
        self.sql_remote_metadata = Some(metadata);
        self
    }

    /// Get OpenSearch namespace from dynamic.ini
    ///
    /// Reads indexing.nodePool.name from dynamic.ini and converts to namespace.
    /// Falls back to elasticsearch-2-namespace if not found (matches Python behavior).
    fn get_opensearch_namespace(&self, staging: bool) -> String {
        let default_namespace = "elasticsearch-2-namespace".to_string();

        let Some(ref dynamic_ini) = self.dynamic_ini else {
            return default_namespace;
        };

        // Get the nodepool key based on staging flag (matches Python IndexingConfig constants)
        let nodepool_key = if staging { "staging.nodePool.name" } else { "nodePool.name" };

        // Try to read nodepool from dynamic.ini
        let nodepool = match dynamic_ini.get("indexing", nodepool_key) {
            Some(np) if !np.is_empty() => np,
            _ => return default_namespace,
        };

        // Convert nodepool to namespace
        match get_elastic_namespace(nodepool) {
            Ok(ns) => ns,
            Err(e) => {
                warn!("Failed to convert nodepool '{}' to namespace: {}", nodepool, e);
                default_namespace
            }
        }
    }

    /// Get AWS bastion instance ID
    ///
    /// Queries AWS EC2 for the bastion instance in the specified region.
    fn get_aws_bastion_instance_id(&self, region: &str) -> Option<String> {
        // Use AWS CLI to describe instances
        println!("   └─ Looking up AWS bastion instance in region: {}", region);
        let output = Command::new("aws")
            .args([
                "ec2",
                "describe-instances",
                "--region",
                region,
                "--filters",
                "Name=tag:Name,Values=bastion",
                "Name=instance-state-name,Values=running",
                "--query",
                "Reservations[0].Instances[0].InstanceId",
                "--output",
                "text",
            ])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let instance_id = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if instance_id.is_empty() || instance_id == "None" {
                    eprintln!("⚠️  No running bastion instance found in region {}", region);
                    None
                } else {
                    println!("   └─ Found bastion instance: {}", instance_id);
                    Some(instance_id)
                }
            }
            Ok(out) => {
                eprintln!(
                    "⚠️  Failed to get bastion instance: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                None
            }
            Err(e) => {
                eprintln!("⚠️  Failed to execute aws cli: {}", e);
                None
            }
        }
    }

    /// Apply platform-specific configuration
    pub fn apply_platform(&mut self, platform: &dyn Platform) {
        let mut temp_profile = TempProfile {
            project: String::new(),
            platform: String::new(),
            zone: String::new(),
            region: String::new(),
            project_number: String::new(),
            account_id: None,
            architecture: self.profile.architecture.clone(),
            devdock_root: self.profile.devdock_root.clone(),
            dns_kubectl: String::new(),
            current_profile: String::new(),
            vault_key: None,
        };

        platform.update_temp_profile(&mut temp_profile);

        self.profile.project = temp_profile.project.clone();
        self.profile.platform = temp_profile.platform.clone();
        self.profile.zone = temp_profile.zone;
        self.profile.region = temp_profile.region;
        self.profile.project_number = temp_profile.project_number;
        self.profile.dns_kubectl = Some(temp_profile.dns_kubectl);

        // For AWS platform, set aws_account_id from temp_profile and vault_key
        if temp_profile.platform == "aws" {
            self.profile.aws_account_id = temp_profile.account_id;
            self.profile.vault_key = "/tmp/stackdriver-exporter-key.json".to_string();
        }
    }

    /// Apply the original profile settings (pipelines, test_suite, env, vault_key)
    pub fn apply_profile_settings(&mut self, profile: &Profile) {
        // Apply vault_key from profile env (defaults to empty string)
        if let Some(vault_key) = profile.env.get("vault_key") {
            self.profile.vault_key = vault_key.clone();
        }

        // Apply top-level environment variables
        if !profile.env.is_empty() {
            self.profile.env = Some(profile.env.clone());
        }

        // Apply debug_mode from QE service if present
        if let Some(qe_config) = profile.services.get("qe") {
            if let Some(debug_val) = qe_config.env.get("DEBUG_MODE") {
                self.profile.debug_mode = debug_val.clone();
            }
        }

        // Apply pipelines
        if !profile.pipelines.is_empty() {
            let mut pipelines_map = HashMap::new();
            for (name, config) in &profile.pipelines {
                let mut value = serde_json::Map::new();
                value.insert(
                    "type".to_string(),
                    serde_json::Value::String(config.pipeline_type.clone()),
                );
                for (k, v) in &config.config {
                    value.insert(k.clone(), v.clone());
                }
                pipelines_map.insert(name.clone(), serde_json::Value::Object(value));
            }
            self.profile.pipelines = Some(pipelines_map);
        }

        // Apply test_suite
        if !profile.test_suite.is_empty() {
            let mut test_suite_map = HashMap::new();
            for (name, config) in &profile.test_suite {
                let mut value = serde_json::Map::new();
                value.insert(
                    "type".to_string(),
                    serde_json::Value::String(config.test_type.clone()),
                );
                for (k, v) in &config.config {
                    value.insert(k.clone(), v.clone());
                }
                test_suite_map.insert(name.clone(), serde_json::Value::Object(value));
            }
            self.profile.test_suite = Some(test_suite_map);
        }
    }

    /// Apply services from dependency graph and original profile
    pub fn apply_services(
        &mut self,
        graph: &DependencyGraph,
        profile: &Profile,
        active_services: Option<&str>,
    ) {
        // Parse active services list
        let active_set: Option<HashSet<&str>> =
            active_services.map(|s| s.split(',').map(str::trim).collect());

        // Check if SQL is local (for docbuilder special case)
        let sql_is_local = profile
            .infra
            .get("sql")
            .is_some_and(|c| c.deployment_type == devdock_core::types::DeploymentType::Local);

        // Track if QE is in the graph
        let has_qe = graph.nodes().any(|n| n.name == "qe");

        // Handle pre_setup and post_setup hooks first - pass them through to temp_profile
        // These are special "services" that Tilt's service_hooks/Tiltfile handles
        for hook_name in &["pre_setup", "post_setup"] {
            if let Some(hook_config) = profile.services.get(*hook_name) {
                self.apply_service_hook(hook_name, hook_config);
            }
        }

        for node in graph.nodes() {
            // Determine if service is active
            let is_active = match node.component_type {
                ComponentType::Service => {
                    match &active_set {
                        Some(set) => set.contains(node.name.as_str()),
                        None => true, // All active if not specified
                    }
                }
                ComponentType::Infra => true,
                _ => true,
            };

            // Get port from env if available (not used in temp_profile but kept for reference)
            let _port: Option<u16> = node.config.env.get("PORT").and_then(|p| p.parse().ok());

            match node.component_type {
                ComponentType::Service => {
                    // Skip pre_setup and post_setup - they're handled separately
                    if node.name == "pre_setup" || node.name == "post_setup" {
                        continue;
                    }

                    // Get original service config from profile for type, env and hot_reload
                    let profile_service = profile.services.get(&node.name);

                    // Use profile's deployment type (local/remote), not devdock_config.json's service type
                    let service_type = if let Some(svc) = profile_service {
                        // Use deployment_type from profile if explicitly specified
                        match svc.deployment_type {
                            devdock_core::types::DeploymentType::Local => "local".to_string(),
                            devdock_core::types::DeploymentType::Remote => "remote".to_string(),
                        }
                    } else {
                        // Fallback to devdock_config.json if not in profile
                        if node.config.is_local() {
                            "local".to_string()
                        } else {
                            "remote".to_string()
                        }
                    };

                    // Start with env from the PROFILE, then inject runtime vars
                    let env = {
                        let mut env = profile_service
                            .and_then(|s| if s.env.is_empty() { None } else { Some(s.env.clone()) })
                            .unwrap_or_default();
                        env.insert("IS_DEVDOCK".to_string(), "true".to_string());
                        let devdock_id = std::env::var("DEVDOCK_ID").unwrap_or_default();
                        if devdock_id.is_empty() {
                            tracing::warn!("DEVDOCK_ID env var is empty during config generation — services will have no session correlation");
                        }
                        env.insert("DEVDOCK_ID".to_string(), devdock_id);
                        Some(env)
                    };

                    let hot_reload = profile_service.and_then(|s| {
                        s.hot_reload.as_ref().map(|hr| match hr {
                            devdock_core::profile::HotReloadConfig::Enabled(b) => {
                                if *b {
                                    "enable".to_string()
                                } else {
                                    "disable".to_string()
                                }
                            }
                            devdock_core::profile::HotReloadConfig::Setting(s) => s.clone(),
                        })
                    });

                    // Extract any extra fields from the profile (like tick_type)
                    let extra = profile_service.map(|s| s.config.clone()).unwrap_or_default();

                    let mut entry = ServiceProfileEntry {
                        service_type,
                        active: is_active,
                        port: None, // Don't include port in temp_profile, Python doesn't
                        env,
                        hot_reload,
                        script: None,
                        service: None,
                        depends_on_services: None,
                        timeout: None,
                        extra,
                    };

                    // Special case: Docbuilder with local SQL - add ENABLE_WRITES env
                    if node.name == "docbuilder" && sql_is_local {
                        let env_map = entry.env.get_or_insert_with(HashMap::new);
                        env_map
                            .insert("ENABLE_WRITES".to_string(), "--allWritesEnabled".to_string());
                    }

                    self.profile.services.insert(node.name.clone(), entry);
                }
                ComponentType::Infra => {
                    self.apply_infra_config(&node.name, profile);
                }
                _ => {}
            }
        }

        // QE should default to remote when not specified in the profile
        // This matches Python behavior in config_util.py:
        //   if 'services' in profile and 'qe' not in comps:
        // Only add QE if the profile already has services AND QE wasn't resolved via dependencies
        let has_services = !profile.services.is_empty();
        if has_services && !has_qe {
            self.profile.services.insert(
                "qe".to_string(),
                ServiceProfileEntry {
                    service_type: "remote".to_string(),
                    active: true, // Remote services are always active
                    port: None,
                    env: None,
                    hot_reload: None,
                    script: None,
                    service: None,
                    depends_on_services: None,
                    timeout: None,
                    extra: HashMap::new(),
                },
            );
        }
    }

    /// Apply infrastructure configuration from the profile
    fn apply_infra_config(&mut self, name: &str, profile: &Profile) {
        // Get infra config from profile, or create a default one for auto-added dependencies
        let infra_config = profile.infra.get(name);

        let infra_type = match infra_config {
            Some(c) => match c.deployment_type {
                devdock_core::types::DeploymentType::Local => "local",
                devdock_core::types::DeploymentType::Remote => "remote",
            },
            // Default to local for auto-added infra dependencies (matches Python behavior)
            None => "local",
        }
        .to_string();

        let mut extra = HashMap::new();

        // Copy all extra config from the profile (if it exists)
        if let Some(config) = infra_config {
            for (key, value) in &config.config {
                extra.insert(key.clone(), value.clone());
            }
        }

        // SQL-specific configuration
        if name == "sql" {
            if infra_type == "local" {
                // Set k8s_service_account for local SQL
                self.profile.k8s_service_account = Some("root".to_string());

                // Set defaults if not present
                if !extra.contains_key("mode") {
                    extra.insert("mode".to_string(), serde_json::Value::String("fast".to_string()));
                }
                if !extra.contains_key("copy_tables") {
                    extra.insert(
                        "copy_tables".to_string(),
                        serde_json::Value::String(String::new()),
                    );
                }
                if !extra.contains_key("full_cleanup") {
                    extra.insert("full_cleanup".to_string(), serde_json::Value::Bool(false));
                }
                if !extra.contains_key("copy_keyval") {
                    extra.insert("copy_keyval".to_string(), serde_json::Value::Bool(true));
                }
                if !extra.contains_key("copy_where_queries") {
                    extra
                        .insert("copy_where_queries".to_string(), serde_json::Value::Array(vec![]));
                }
                if !extra.contains_key("use_full_instance_export") {
                    extra.insert(
                        "use_full_instance_export".to_string(),
                        serde_json::Value::Bool(false),
                    );
                }
                if !extra.contains_key("use_mysql_seed") {
                    extra.insert("use_mysql_seed".to_string(), serde_json::Value::Bool(false));
                }
            } else {
                // Remote SQL configuration
                // Don't include copy_tables/mode/etc for remote SQL
                extra.retain(|k, _| {
                    !matches!(
                        k.as_str(),
                        "copy_tables"
                            | "mode"
                            | "full_cleanup"
                            | "copy_keyval"
                            | "copy_where_queries"
                    )
                });
                self.apply_remote_sql_config(&mut extra);
            }
        }

        // OpenSearch-specific configuration
        if name == "opensearch" {
            if !extra.contains_key("full_cleanup") {
                extra.insert("full_cleanup".to_string(), serde_json::Value::Bool(false));
            }
            // For remote OpenSearch (non-Azure), add namespace from dynamic.ini
            // Azure doesn't use namespace-based OpenSearch config
            let platform = self.profile.platform.as_str();
            if infra_type == "remote" && platform != "azure" && !extra.contains_key("namespace") {
                // Check if staging mode from config
                let staging =
                    extra.get("staging").and_then(serde_json::Value::as_bool).unwrap_or(false);

                let namespace = self.get_opensearch_namespace(staging);
                extra.insert("namespace".to_string(), serde_json::Value::String(namespace));
            }
        }

        // Bastion-specific configuration
        if name == "bastion" {
            // Add local_to_remote_ports for port forwarding
            // Default ports: [7972, 8888] for Tilt UI forwarding
            if !extra.contains_key("local_to_remote_ports") {
                extra
                    .insert("local_to_remote_ports".to_string(), serde_json::json!([[7972, 8888]]));
            }

            // For AWS platform, get bastion instance ID
            let platform = self.profile.platform.as_str();
            if platform == "aws" && !extra.contains_key("bastion_instance_id") {
                let region = &self.profile.region;
                if let Some(instance_id) = self.get_aws_bastion_instance_id(region) {
                    extra.insert(
                        "bastion_instance_id".to_string(),
                        serde_json::Value::String(instance_id),
                    );
                }
            }
        }

        // Redis-specific - ensure it's added even with minimal config
        if name == "redis" {
            // Redis doesn't need extra fields, just type
        }

        let entry = InfraProfileEntry { infra_type, extra };

        self.profile.infra.insert(name.to_string(), entry);
    }

    /// Apply service hook configuration (pre_setup or post_setup)
    ///
    /// These hooks are special "services" that Tilt's service_hooks/Tiltfile handles.
    /// We pass them through to temp_profile.yaml with all their configuration intact.
    fn apply_service_hook(&mut self, hook_name: &str, hook_config: &devdock_core::ServiceConfig) {
        use serde_json::json;

        // Create a JSON object with all the hook configuration
        let mut hook_data = serde_json::Map::new();

        // Always set type (usually "local")
        hook_data.insert(
            "type".to_string(),
            json!(match hook_config.deployment_type {
                devdock_core::types::DeploymentType::Local => "local",
                devdock_core::types::DeploymentType::Remote => "remote",
            }),
        );

        // Add script if present
        if let Some(ref script) = hook_config.script {
            hook_data.insert("script".to_string(), json!(script));
        }

        // Add target service if present
        if let Some(ref service) = hook_config.service {
            hook_data.insert("service".to_string(), json!(service));
        }

        // Add depends_on_services if present
        if let Some(ref deps) = hook_config.depends_on_services {
            hook_data.insert("depends_on_services".to_string(), json!(deps));
        }

        // Add timeout if present
        if let Some(ref timeout) = hook_config.timeout {
            hook_data.insert("timeout".to_string(), json!(timeout));
        }

        // Add environment variables if present
        if !hook_config.env.is_empty() {
            hook_data.insert("env".to_string(), json!(hook_config.env));
        }

        // Add any other configuration from the flattened config map
        for (key, value) in &hook_config.config {
            if !hook_data.contains_key(key) {
                hook_data.insert(key.clone(), value.clone());
            }
        }

        // Convert to ServiceProfileEntry format for serialization
        // Note: We can't use ServiceProfileEntry directly because it doesn't have all the hook fields
        // So we'll store it as a JSON value in the services map
        let entry_json = serde_json::Value::Object(hook_data);

        // Deserialize into a ServiceProfileEntry with extra fields flattened
        // This is a bit of a hack but allows us to preserve all hook configuration
        if let Ok(_hook_value) = serde_json::from_value::<serde_json::Value>(entry_json.clone()) {
            // Store the hook configuration directly in services as a raw JSON value
            // We'll need to modify TempProfileData to support this
            self.store_hook_config(hook_name, entry_json);
        }
    }

    /// Store hook configuration as raw JSON (workaround for hook-specific fields)
    fn store_hook_config(&mut self, hook_name: &str, hook_data: serde_json::Value) {
        // Extract all fields from the hook configuration
        let service_type =
            hook_data.get("type").and_then(|v| v.as_str()).unwrap_or("local").to_string();

        let env = hook_data
            .get("env")
            .and_then(|v| serde_json::from_value::<HashMap<String, String>>(v.clone()).ok());

        let script =
            hook_data.get("script").and_then(|v| v.as_str()).map(std::string::ToString::to_string);

        let service =
            hook_data.get("service").and_then(|v| v.as_str()).map(std::string::ToString::to_string);

        let depends_on_services = hook_data
            .get("depends_on_services")
            .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok());

        let timeout =
            hook_data.get("timeout").and_then(|v| v.as_str()).map(std::string::ToString::to_string);

        // Collect any extra fields that aren't in the standard set
        let standard_keys = ["type", "env", "script", "service", "depends_on_services", "timeout"];
        let mut extra = HashMap::new();
        if let Some(obj) = hook_data.as_object() {
            for (key, value) in obj {
                if !standard_keys.contains(&key.as_str()) {
                    extra.insert(key.clone(), value.clone());
                }
            }
        }

        let entry = ServiceProfileEntry {
            service_type,
            active: true, // Hooks are always active
            port: None,
            env,
            hot_reload: None,
            script,
            service,
            depends_on_services,
            timeout,
            extra,
        };

        // Store in the services map
        self.profile.services.insert(hook_name.to_string(), entry);
    }

    /// Set the multi-project configuration (glean-apps-canary)
    pub fn set_multi_project(&mut self, multi_project: MultiProjectProfile) {
        self.profile.multi_project = Some(multi_project);
    }

    /// Inject multi-project services into the top-level services map
    /// so they appear as first-class Tilt resources.
    pub fn inject_top_level_services(&mut self, services: HashMap<String, ServiceProfileEntry>) {
        for (name, entry) in services {
            self.profile.services.insert(name, entry);
        }
    }

    /// Apply remote SQL configuration to temp_profile (matches Python logic)
    fn apply_remote_sql_config(&mut self, extra: &mut HashMap<String, serde_json::Value>) {
        let Some(ref metadata) = self.sql_remote_metadata else {
            warn!("No SQL metadata available for remote SQL configuration");
            return;
        };

        let platform = self.profile.platform.as_str();
        let connection_mode =
            extra.get("connection_mode").and_then(serde_json::Value::as_str).unwrap_or("auto");
        if platform == "google" && connection_mode == "kubernetes" {
            extra.insert(
                "kubernetes_proxy".to_string(),
                serde_json::json!({
                    "target": "deployment/admin-deployment",
                    "namespace": "default",
                    "port_forwards": [[3307, 13306], [3308, 13307]],
                }),
            );
            self.profile.sql_account = Some("admin-console".to_string());
            return;
        }

        // Primary-project path never runs as multi-project: the nested glean_apps_canary
        // case is handled separately by `build_remote_sql_config` in generator.rs.
        let outputs = build_remote_sql_config(metadata, self.dynamic_ini.as_ref(), platform, false);

        for (k, v) in outputs.sql_extras {
            extra.insert(k, v);
        }

        if !outputs.bastion_port_forwards.is_empty() {
            self.profile.infra.entry("bastion".to_string()).or_insert_with(|| InfraProfileEntry {
                infra_type: "local".to_string(),
                extra: HashMap::new(),
            });

            if let Some(bastion) = self.profile.infra.get_mut("bastion") {
                apply_bastion_port_forwards(&mut bastion.extra, &outputs.bastion_port_forwards);
            }
        }

        if let Some(acct) = outputs.sql_account {
            self.profile.sql_account = Some(acct);
        }
        if let Some(sa) = outputs.k8s_service_account {
            self.profile.k8s_service_account = Some(sa);
        }
    }

    /// Generate the final YAML string
    pub fn generate(self) -> Result<String> {
        serde_yaml::to_string(&self.profile)
            .map_err(|e| anyhow::anyhow!("Failed to serialize temp profile: {e}"))
    }
}

/// Outputs produced by `build_remote_sql_config`.
///
/// Shared by the primary-project path (applied to `TempProfileData`) and the
/// multi-project path (applied to `MultiProjectProfile`). Callers are
/// responsible for writing the outputs onto the right target, because the
/// nested `glean_apps_canary` profile does not own `sql_account` /
/// `k8s_service_account` today — those stay on the outer profile.
pub(crate) struct RemoteSqlOutputs {
    /// Fields to merge into the `sql` infra entry's flattened extras
    /// (e.g. `cloud_sql_args`, `bastion`, `instances`, `ports`,
    /// `use_iam_auth`, `connections`).
    pub sql_extras: HashMap<String, serde_json::Value>,
    /// Port forwards to append to the bastion infra entry's
    /// `local_to_remote_ports`. Each inner vec is `[local_port, remote_port]`.
    pub bastion_port_forwards: Vec<Vec<u16>>,
    /// sql_account to set on the containing profile (GCP IAM auth only).
    pub sql_account: Option<String>,
    /// k8s_service_account to set on the containing profile (Azure only).
    pub k8s_service_account: Option<String>,
}

/// Pure helper that mirrors Python's `SqlServiceDependency.update_temp_profile`
/// for the remote-SQL branch. No side effects on a generator instance — the
/// caller applies the returned outputs to either the top-level profile or the
/// nested multi-project profile.
pub(crate) fn build_remote_sql_config(
    metadata: &crate::sql::SqlRemoteMetadata,
    dynamic_ini: Option<&ParsedIniConfig>,
    platform: &str,
    is_multi_project: bool,
) -> RemoteSqlOutputs {
    let mut sql_extras: HashMap<String, serde_json::Value> = HashMap::new();
    let mut bastion_port_forwards: Vec<Vec<u16>> = Vec::new();
    let mut sql_account: Option<String> = None;
    let mut k8s_service_account: Option<String> = None;

    match platform {
        "azure" => {
            let ports: Vec<u16> = vec![3407, 3408];
            sql_extras.insert("bastion".to_string(), serde_json::json!(true));

            let instances: Vec<String> = metadata.instances.keys().cloned().collect();
            sql_extras.insert("instances".to_string(), serde_json::json!(instances));
            sql_extras.insert("ports".to_string(), serde_json::json!(ports));

            bastion_port_forwards.push(vec![3407, 7972]);
            bastion_port_forwards.push(vec![3408, 7973]);

            k8s_service_account = Some("mysqladmin".to_string());
        }
        "aws" => {
            let curr_port_base: u16 = if is_multi_project { 3311 } else { 3307 };
            let mut connections: Vec<String> = Vec::new();
            let mut ports: Vec<u16> = Vec::new();

            if let Some(ini) = dynamic_ini {
                for (idx, instance_name) in
                    ["frontendInstance", "backendInstance"].iter().enumerate()
                {
                    let key = format!("{}.aws.instanceEndpoint", instance_name);
                    if let Some(endpoint) = ini.get("sql", &key) {
                        connections.push(endpoint.clone());
                        ports.push(curr_port_base + idx as u16);
                    }
                }
            }

            sql_extras.insert("connections".to_string(), serde_json::json!(connections));
            sql_extras.insert("ports".to_string(), serde_json::json!(ports));
        }
        "google" => {
            let curr_port_base: u16 = if is_multi_project { 3311 } else { 3307 };

            if metadata.use_iam_auth {
                sql_account = Some(crate::constants::COMPUTE_ENGINE_SERVICE_ACCOUNT.to_string());
            }

            // Ordered list of active SQL instance types for this project.
            //
            // Python's SqlServiceDependency iterates `self.instances` in dict
            // insertion order, which comes from `sql.active.instances`. For
            // glean-apps-canary that's `internal,frontendInstance`, not the
            // default `frontendInstance,backendInstance`. Re-derive it from
            // the INI instead of hardcoding so multi-project projects emit
            // their correct instance set.
            let ordered_instances: Vec<String> = dynamic_ini
                .and_then(|ini| ini.get("sql", "active.instances"))
                .map(|s| s.split(',').map(|v| v.trim().to_string()).collect())
                .unwrap_or_else(|| {
                    vec!["frontendInstance".to_string(), "backendInstance".to_string()]
                });

            if metadata.use_public_ip || !metadata.is_bastion_available {
                // Path 1: cloud-sql-proxy.
                let mut cloud_sql_args: Vec<String> = Vec::new();
                let mut curr_port = curr_port_base;
                for instance_name in &ordered_instances {
                    if let Some(connection_str) = metadata.instances.get(instance_name) {
                        if !connection_str.is_empty() {
                            cloud_sql_args.push(format!("{}?port={}", connection_str, curr_port));
                            curr_port += 1;
                        }
                    }
                }
                if metadata.use_iam_auth {
                    cloud_sql_args.push("--auto-iam-authn".to_string());
                }
                sql_extras.insert("cloud_sql_args".to_string(), serde_json::json!(cloud_sql_args));
            } else {
                // Path 2: bastion tunnel. Order must match the INI generator.
                sql_extras.insert("bastion".to_string(), serde_json::json!(true));
                sql_extras
                    .insert("use_iam_auth".to_string(), serde_json::json!(metadata.use_iam_auth));

                let mut instances_list: Vec<String> = Vec::new();
                let mut ports: Vec<u16> = Vec::new();
                let mut curr_port = curr_port_base;
                for instance_name in &ordered_instances {
                    if metadata.instances.contains_key(instance_name) {
                        instances_list.push(instance_name.clone());
                        ports.push(curr_port);
                        curr_port += 1;
                    }
                }

                let bastion_port_map = dynamic_ini
                    .map(|ini| crate::sql::get_bastion_port_map(ini, metadata.use_iam_auth))
                    .unwrap_or_default();
                let remote_port_base = if metadata.use_iam_auth {
                    crate::constants::IAM_PORT_START
                } else {
                    crate::constants::PORT_START
                };
                for (i, instance_name) in instances_list.iter().enumerate() {
                    let remote_port = bastion_port_map
                        .get(instance_name)
                        .copied()
                        .unwrap_or(remote_port_base + i as u16);
                    bastion_port_forwards.push(vec![ports[i], remote_port]);
                }

                sql_extras.insert("instances".to_string(), serde_json::json!(instances_list));
                sql_extras.insert("ports".to_string(), serde_json::json!(ports));
            }
        }
        _ => {}
    }

    RemoteSqlOutputs { sql_extras, bastion_port_forwards, sql_account, k8s_service_account }
}

/// Append `forwards` to the `local_to_remote_ports` list on a bastion infra
/// entry's flattened extras. Creates the key if it doesn't exist.
pub(crate) fn apply_bastion_port_forwards(
    bastion_extra: &mut HashMap<String, serde_json::Value>,
    forwards: &[Vec<u16>],
) {
    let mut local_to_remote = bastion_extra
        .get("local_to_remote_ports")
        .and_then(|v| serde_json::from_value::<Vec<Vec<u16>>>(v.clone()).ok())
        .unwrap_or_default();
    local_to_remote.extend(forwards.iter().cloned());
    bastion_extra.insert("local_to_remote_ports".to_string(), serde_json::json!(local_to_remote));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ini_parser::ParsedIniConfig;
    use std::path::PathBuf;

    fn private_google_sql_metadata() -> crate::sql::SqlRemoteMetadata {
        crate::sql::SqlRemoteMetadata {
            instances: HashMap::from([
                (
                    "frontendInstance".to_string(),
                    "glean-salessavvy2:us-central1:sql-frontend".to_string(),
                ),
                (
                    "backendInstance".to_string(),
                    "glean-salessavvy2:us-central1:sql-backend".to_string(),
                ),
            ]),
            use_public_ip: false,
            is_bastion_available: true,
            bastion_zone: Some("us-central1-a".to_string()),
            use_iam_auth: true,
            platform: "google".to_string(),
            project: "glean-salessavvy2".to_string(),
        }
    }

    #[test]
    fn test_kubernetes_remote_sql_uses_admin_sidecar_without_bastion() {
        let profile: Profile = serde_yaml::from_str(
            r#"
project: glean-salessavvy2
infra:
  sql:
    type: remote
    connection_mode: kubernetes
"#,
        )
        .unwrap();
        let mut generator = TempProfileGenerator::new(&PathBuf::from("/tmp"), None)
            .with_sql_metadata(private_google_sql_metadata());
        generator.profile.platform = "google".to_string();
        generator.profile.dns_kubectl = Some("enabled".to_string());

        generator.apply_infra_config("sql", &profile);

        assert!(!generator.profile.infra.contains_key("bastion"));
        assert_eq!(generator.profile.sql_account.as_deref(), Some("admin-console"));
        assert_eq!(
            generator.profile.infra["sql"].extra["kubernetes_proxy"],
            serde_json::json!({
                "target": "deployment/admin-deployment",
                "namespace": "default",
                "port_forwards": [[3307, 13306], [3308, 13307]],
            })
        );
    }

    #[test]
    fn test_opensearch_namespace_default() {
        // When dynamic.ini is not available, should default to elasticsearch-2-namespace
        let generator = TempProfileGenerator::new(&PathBuf::from("/tmp"), None);
        let namespace = generator.get_opensearch_namespace(false);
        assert_eq!(
            namespace, "elasticsearch-2-namespace",
            "Should default to elasticsearch-2-namespace when dynamic.ini is not available"
        );
    }

    #[test]
    fn test_opensearch_namespace_from_dynamic_ini() {
        // When dynamic.ini has nodePool.name, should convert to namespace
        let mut dynamic_ini = ParsedIniConfig::default();
        dynamic_ini.set("indexing", "nodePool.name", "elastic-node-pool-3");

        let generator =
            TempProfileGenerator::new(&PathBuf::from("/tmp"), None).with_dynamic_ini(dynamic_ini);

        let namespace = generator.get_opensearch_namespace(false);
        assert_eq!(
            namespace, "elasticsearch-3-namespace",
            "Should convert elastic-node-pool-3 to elasticsearch-3-namespace"
        );
    }

    #[test]
    fn test_opensearch_namespace_staging() {
        // Test staging nodepool key
        let mut dynamic_ini = ParsedIniConfig::default();
        dynamic_ini.set("indexing", "staging.nodePool.name", "elastic-node-pool-5");

        let generator =
            TempProfileGenerator::new(&PathBuf::from("/tmp"), None).with_dynamic_ini(dynamic_ini);

        let namespace = generator.get_opensearch_namespace(true);
        assert_eq!(
            namespace, "elasticsearch-5-namespace",
            "Should convert staging.nodePool.name to correct namespace"
        );
    }

    #[test]
    fn test_opensearch_namespace_invalid_nodepool() {
        // When nodepool is invalid, should fall back to default
        let mut dynamic_ini = ParsedIniConfig::default();
        dynamic_ini.set("indexing", "nodePool.name", "invalid-pool");

        let generator =
            TempProfileGenerator::new(&PathBuf::from("/tmp"), None).with_dynamic_ini(dynamic_ini);

        let namespace = generator.get_opensearch_namespace(false);
        assert_eq!(
            namespace, "elasticsearch-2-namespace",
            "Should fall back to default when nodepool is invalid"
        );
    }

    #[test]
    fn test_opensearch_namespace_empty_nodepool() {
        // When nodepool is empty, should use default
        let mut dynamic_ini = ParsedIniConfig::default();
        dynamic_ini.set("indexing", "nodePool.name", "");

        let generator =
            TempProfileGenerator::new(&PathBuf::from("/tmp"), None).with_dynamic_ini(dynamic_ini);

        let namespace = generator.get_opensearch_namespace(false);
        assert_eq!(
            namespace, "elasticsearch-2-namespace",
            "Should use default when nodepool is empty"
        );
    }

    #[test]
    fn test_inject_top_level_services_adds_multi_project_services() {
        let mut generator = TempProfileGenerator::new(&PathBuf::from("/tmp"), None);

        // Simulate a top-level service already present
        generator.profile.services.insert(
            "qe".to_string(),
            ServiceProfileEntry {
                service_type: "local".to_string(),
                active: true,
                port: None,
                env: None,
                hot_reload: None,
                script: None,
                service: None,
                depends_on_services: None,
                timeout: None,
                extra: HashMap::new(),
            },
        );

        // Inject a multi-project service (e.g., central_datasources from glean-apps-canary)
        let mut multi_services = HashMap::new();
        let mut env = HashMap::new();
        env.insert("USE_DEVDOCK_INI".to_string(), "true".to_string());
        multi_services.insert(
            "central_datasources".to_string(),
            ServiceProfileEntry {
                service_type: "local".to_string(),
                active: true,
                port: None,
                env: Some(env),
                hot_reload: Some("disable".to_string()),
                script: None,
                service: None,
                depends_on_services: None,
                timeout: None,
                extra: HashMap::new(),
            },
        );

        generator.inject_top_level_services(multi_services);

        // Both services should now be in top-level services
        assert!(
            generator.profile.services.contains_key("qe"),
            "Original top-level service should still be present"
        );
        assert!(
            generator.profile.services.contains_key("central_datasources"),
            "Multi-project service should be promoted to top-level"
        );

        let cd = generator.profile.services.get("central_datasources").unwrap();
        assert_eq!(cd.service_type, "local");
        assert!(cd.active);
        assert_eq!(cd.hot_reload.as_deref(), Some("disable"));
    }
}
