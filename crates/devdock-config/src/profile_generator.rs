//! Temporary profile generator for Tilt.

use crate::dependency_graph::{DependencyGraph, ComponentType};
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

#[derive(Debug, Serialize)]
pub struct TempProfileData {
    pub project: String,
    pub platform: String,
    pub devdock_root: String,
    pub region: String,
    pub zone: String,
    pub architecture: String,
    pub project_number: String,
    pub enable_service_metrics: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_kubectl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub k8s_service_account: Option<String>,
    pub debug_mode: String,
    pub vault_key: String,
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
            current_profile: profile_path.map(|s| s.to_string()),
            dns_kubectl: Some("disabled".to_string()),
            k8s_service_account: None,
            debug_mode: "False".to_string(),
            vault_key: String::new(),
            services: HashMap::new(),
            infra: HashMap::new(),
            pipelines: None,
            test_suite: None,
            env: None,
            multi_project: None,
        };

        Self { 
            profile,
            dynamic_ini: None,
            sql_remote_metadata: None,
        }
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
    /// Reads indexing.index.nodePool from dynamic.ini and converts to namespace.
    /// Falls back to elasticsearch-1-namespace if not found.
    fn get_opensearch_namespace(&self, staging: bool) -> String {
        let default_namespace = "elasticsearch-1-namespace".to_string();

        let Some(ref dynamic_ini) = self.dynamic_ini else {
            return default_namespace;
        };

        // Get the nodepool key based on staging flag
        let nodepool_key = if staging {
            "index.staging.nodePool"
        } else {
            "index.nodePool"
        };

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
        let output = Command::new("aws")
            .args([
                "ec2",
                "describe-instances",
                "--region", region,
                "--filters",
                "Name=tag:Name,Values=bastion",
                "Name=instance-state-name,Values=running",
                "--query", "Reservations[0].Instances[0].InstanceId",
                "--output", "text",
            ])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let instance_id = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if instance_id.is_empty() || instance_id == "None" {
                    warn!("No running bastion instance found in region {}", region);
                    None
                } else {
                    Some(instance_id)
                }
            }
            Ok(out) => {
                warn!(
                    "Failed to get bastion instance: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                None
            }
            Err(e) => {
                warn!("Failed to execute aws cli: {}", e);
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
            architecture: self.profile.architecture.clone(),
            devdock_root: self.profile.devdock_root.clone(),
            dns_kubectl: String::new(),
            current_profile: String::new(),
        };

        platform.update_temp_profile(&mut temp_profile);

        self.profile.project = temp_profile.project;
        self.profile.platform = temp_profile.platform;
        self.profile.zone = temp_profile.zone;
        self.profile.region = temp_profile.region;
        self.profile.project_number = temp_profile.project_number;
        self.profile.dns_kubectl = Some(temp_profile.dns_kubectl);
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
                value.insert("type".to_string(), serde_json::Value::String(config.pipeline_type.clone()));
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
                value.insert("type".to_string(), serde_json::Value::String(config.test_type.clone()));
                for (k, v) in &config.config {
                    value.insert(k.clone(), v.clone());
                }
                test_suite_map.insert(name.clone(), serde_json::Value::Object(value));
            }
            self.profile.test_suite = Some(test_suite_map);
        }
    }

    /// Apply services from dependency graph and original profile
    pub fn apply_services(&mut self, graph: &DependencyGraph, profile: &Profile, active_services: Option<&str>) {
        // Parse active services list
        let active_set: Option<HashSet<&str>> = active_services
            .map(|s| s.split(',').map(|s| s.trim()).collect());

        // Check if SQL is local (for docbuilder special case)
        let sql_is_local = profile.infra.get("sql")
            .map(|c| c.deployment_type == devdock_core::types::DeploymentType::Local)
            .unwrap_or(false);

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
            let _port: Option<u16> = node.config.env.get("PORT")
                .and_then(|p| p.parse().ok());

            match node.component_type {
                ComponentType::Service => {
                    // Skip pre_setup and post_setup - they're handled separately
                    if node.name == "pre_setup" || node.name == "post_setup" {
                        continue;
                    }

                    // Get original service config from profile for type, env and hot_reload
                    let profile_service = profile.services.get(&node.name);

                    // Use profile's deployment type (local/remote), not devdock_config.json's service type
                    let service_type = if node.config.is_local() {
                        "local".to_string()
                    } else {
                        "remote".to_string()
                    };

                    // Only include env from the PROFILE, not from devdock_config.json
                    let env = profile_service.and_then(|s| {
                        if s.env.is_empty() { None } else { Some(s.env.clone()) }
                    });

                    let hot_reload = profile_service.and_then(|s| {
                        s.hot_reload.as_ref().map(|hr| match hr {
                            devdock_core::profile::HotReloadConfig::Enabled(b) => {
                                if *b { "enable".to_string() } else { "disable".to_string() }
                            }
                            devdock_core::profile::HotReloadConfig::Setting(s) => s.clone(),
                        })
                    });

                    // Extract any extra fields from the profile (like tick_type)
                    let extra = profile_service
                        .map(|s| s.config.clone())
                        .unwrap_or_default();

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
                        env_map.insert("ENABLE_WRITES".to_string(), "--allWritesEnabled".to_string());
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
            self.profile.services.insert("qe".to_string(), ServiceProfileEntry {
                service_type: "remote".to_string(),
                active: true,  // Remote services are always active
                port: None,
                env: None,
                hot_reload: None,
                script: None,
                service: None,
                depends_on_services: None,
                timeout: None,
                extra: HashMap::new(),
            });
        }
    }

    /// Apply infrastructure configuration from the profile
    fn apply_infra_config(&mut self, name: &str, profile: &Profile) {
        let infra_config = match profile.infra.get(name) {
            Some(c) => c,
            None => return,
        };

        let infra_type = match infra_config.deployment_type {
            devdock_core::types::DeploymentType::Local => "local",
            devdock_core::types::DeploymentType::Remote => "remote",
        }.to_string();

        let mut extra = HashMap::new();

        // Copy all extra config from the profile
        for (key, value) in &infra_config.config {
            extra.insert(key.clone(), value.clone());
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
                    extra.insert("copy_tables".to_string(), serde_json::Value::String(String::new()));
                }
                if !extra.contains_key("full_cleanup") {
                    extra.insert("full_cleanup".to_string(), serde_json::Value::Bool(false));
                }
                if !extra.contains_key("copy_keyval") {
                    extra.insert("copy_keyval".to_string(), serde_json::Value::Bool(true));
                }
                if !extra.contains_key("copy_where_queries") {
                    extra.insert("copy_where_queries".to_string(), serde_json::Value::Array(vec![]));
                }
            } else {
                // Remote SQL configuration
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
                let staging = extra.get("staging")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                
                let namespace = self.get_opensearch_namespace(staging);
                extra.insert(
                    "namespace".to_string(),
                    serde_json::Value::String(namespace),
                );
            }
        }

        // Bastion-specific configuration
        if name == "bastion" {
            // Add local_to_remote_ports for port forwarding
            // Default ports: [7972, 8888] for Tilt UI forwarding
            if !extra.contains_key("local_to_remote_ports") {
                extra.insert(
                    "local_to_remote_ports".to_string(),
                    serde_json::json!([[7972, 8888]]),
                );
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

        let entry = InfraProfileEntry {
            infra_type,
            extra,
        };

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
            })
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
        let service_type = hook_data
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("local")
            .to_string();
        
        let env = hook_data
            .get("env")
            .and_then(|v| serde_json::from_value::<HashMap<String, String>>(v.clone()).ok());

        let script = hook_data
            .get("script")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let service = hook_data
            .get("service")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let depends_on_services = hook_data
            .get("depends_on_services")
            .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok());

        let timeout = hook_data
            .get("timeout")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

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

    /// Add a service to the TOP-LEVEL services dict (used for multi-project services)
    pub fn add_service(&mut self, name: &str, entry: ServiceProfileEntry) {
        self.profile.services.insert(name.to_string(), entry);
    }

    /// Apply remote SQL configuration to temp_profile (matches Python logic)
    fn apply_remote_sql_config(&mut self, extra: &mut HashMap<String, serde_json::Value>) {
        let Some(ref metadata) = self.sql_remote_metadata else {
            warn!("No SQL metadata available for remote SQL configuration");
            return;
        };

        let platform = self.profile.platform.as_str();
        
        // Check if this is multi-project (nested under glean_apps_canary)
        let is_multi_project = self.profile.multi_project.is_some();
        
        match platform {
            "azure" => {
                // Azure: Fixed ports with bastion
                let ports = vec![3407, 3408];
                extra.insert("ports".to_string(), serde_json::json!(ports));
                extra.insert("bastion".to_string(), serde_json::json!(true));
                
                let instances: Vec<String> = metadata.instances.keys().cloned().collect();
                extra.insert("instances".to_string(), serde_json::json!(instances));
                
                // Update bastion config
                self.profile.infra.entry("bastion".to_string()).or_insert_with(|| {
                    InfraProfileEntry {
                        infra_type: "local".to_string(),
                        extra: HashMap::new(),
                    }
                });
                
                if let Some(bastion) = self.profile.infra.get_mut("bastion") {
                    let mut local_to_remote = bastion.extra
                        .get("local_to_remote_ports")
                        .and_then(|v| serde_json::from_value::<Vec<Vec<u16>>>(v.clone()).ok())
                        .unwrap_or_default();
                    
                    local_to_remote.push(vec![3407, 7972]);
                    local_to_remote.push(vec![3408, 7973]);
                    
                    bastion.extra.insert(
                        "local_to_remote_ports".to_string(),
                        serde_json::json!(local_to_remote),
                    );
                }
                
                self.profile.k8s_service_account = Some("mysqladmin".to_string());
            }
            "aws" => {
                // AWS: Direct RDS connections
                let curr_port_base = if is_multi_project { 3311 } else { 3307 };
                let mut connections = Vec::new();
                let mut ports = Vec::new();
                
                if let Some(ref dynamic_ini) = self.dynamic_ini {
                    for (idx, instance_name) in ["frontendInstance", "backendInstance"].iter().enumerate() {
                        let key = format!("{}.aws.instanceEndpoint", instance_name);
                        if let Some(endpoint) = dynamic_ini.get("sql", &key) {
                            connections.push(endpoint.clone());
                            ports.push(curr_port_base + idx as u16);
                        }
                    }
                }
                
                extra.insert("connections".to_string(), serde_json::json!(connections));
                extra.insert("ports".to_string(), serde_json::json!(ports));
            }
            "google" => {
                // GCP: Two paths - cloud-sql-proxy or bastion
                let curr_port_base = if is_multi_project { 3311 } else { 3307 };
                
                if metadata.use_public_ip || !metadata.is_bastion_available {
                    // Path 1: Use cloud-sql-proxy
                    let mut cloud_sql_args = Vec::new();
                    let mut curr_port = curr_port_base;
                    
                    for connection_str in metadata.instances.values() {
                        if !connection_str.is_empty() {
                            cloud_sql_args.push(format!("{}?port={}", connection_str, curr_port));
                            curr_port += 1;
                        }
                    }
                    
                    if metadata.use_iam_auth {
                        cloud_sql_args.push("--auto-iam-authn".to_string());
                    }
                    
                    extra.insert("cloud_sql_args".to_string(), serde_json::json!(cloud_sql_args));
                } else {
                    // Path 2: Use bastion tunnel
                    extra.insert("bastion".to_string(), serde_json::json!(true));
                    extra.insert("use_iam_auth".to_string(), serde_json::json!(metadata.use_iam_auth));
                    
                    let instances: Vec<String> = metadata.instances.keys().cloned().collect();
                    extra.insert("instances".to_string(), serde_json::json!(instances));
                    
                    // Build ports list
                    let mut ports = Vec::new();
                    let mut curr_port = curr_port_base;
                    for _ in &metadata.instances {
                        ports.push(curr_port);
                        curr_port += 1;
                    }
                    extra.insert("ports".to_string(), serde_json::json!(ports));
                    
                    // Update bastion config with port forwarding
                    if let Some(ref dynamic_ini) = self.dynamic_ini {
                        let bastion_port_map = crate::sql::get_bastion_port_map(dynamic_ini, metadata.use_iam_auth);
                        
                        self.profile.infra.entry("bastion".to_string()).or_insert_with(|| {
                            InfraProfileEntry {
                                infra_type: "local".to_string(),
                                extra: HashMap::new(),
                            }
                        });
                        
                        if let Some(bastion) = self.profile.infra.get_mut("bastion") {
                            let mut local_to_remote = bastion.extra
                                .get("local_to_remote_ports")
                                .and_then(|v| serde_json::from_value::<Vec<Vec<u16>>>(v.clone()).ok())
                                .unwrap_or_default();
                            
                            // Add SQL port mappings
                            for (i, instance_name) in metadata.instances.keys().enumerate() {
                                let local_port = ports[i];
                                if let Some(&remote_port) = bastion_port_map.get(instance_name) {
                                    local_to_remote.push(vec![local_port, remote_port]);
                                }
                            }
                            
                            bastion.extra.insert(
                                "local_to_remote_ports".to_string(),
                                serde_json::json!(local_to_remote),
                            );
                        }
                        
                        // Set sql_account for IAM auth
                        if metadata.use_iam_auth {
                            if let Some(account) = dynamic_ini.get("sql", "serviceAccount") {
                                self.profile.vault_key = account.clone();
                            }
                        }
                    }
                }
                
                // Special handling for glean-apps-canary and scio-apps
                if matches!(metadata.project.as_str(), "glean-apps-canary" | "scio-apps") {
                    // These projects use simplified config
                    // The actual db.host is already set in devdock.ini
                }
            }
            _ => {}
        }
    }

    /// Generate the final YAML string
    pub fn generate(self) -> Result<String> {
        serde_yaml::to_string(&self.profile)
            .map_err(|e| anyhow::anyhow!("Failed to serialize temp profile: {}", e))
    }
}

