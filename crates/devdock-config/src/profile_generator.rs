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
}

#[derive(Debug, Serialize)]
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
        };

        Self { 
            profile,
            dynamic_ini: None,
        }
    }

    /// Set the dynamic.ini config for reading production values
    pub fn with_dynamic_ini(mut self, dynamic_ini: ParsedIniConfig) -> Self {
        self.dynamic_ini = Some(dynamic_ini);
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
        match get_elastic_namespace(&nodepool) {
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

                    let mut entry = ServiceProfileEntry {
                        service_type,
                        active: is_active,
                        port: None, // Don't include port in temp_profile, Python doesn't
                        env,
                        hot_reload,
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

        // SQL-specific defaults for local
        if name == "sql" && infra_type == "local" {
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

    /// Generate the final YAML string
    pub fn generate(self) -> Result<String> {
        serde_yaml::to_string(&self.profile)
            .map_err(|e| anyhow::anyhow!("Failed to serialize temp profile: {}", e))
    }
}

