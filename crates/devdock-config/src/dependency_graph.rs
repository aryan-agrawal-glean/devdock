//! Dependency graph for services and infrastructure.

use crate::service_config::ServiceConfig;
use anyhow::{Context, Result};
use devdock_core::profile::{InfraConfig, Profile, ServiceConfig as ProfileServiceConfig};
use devdock_core::types::DeploymentType;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// A node in the dependency graph
#[derive(Debug, Clone)]
pub struct ServiceNode {
    pub name: String,
    pub config: ServiceConfig,
    pub component_type: ComponentType,
}

/// Type of component
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentType {
    Service,
    Infra,
    Pipeline,
    TestSuite,
}

/// Dependency graph for all components
#[derive(Debug)]
pub struct DependencyGraph {
    nodes: HashMap<String, ServiceNode>,
}

impl DependencyGraph {
    /// Build dependency graph from profile, resolving transitive dependencies
    pub async fn build(profile: &Profile, workspace_root: &Path) -> Result<Self> {
        let total_start = std::time::Instant::now();
        let mut graph = Self { nodes: HashMap::new() };

        // Track services explicitly listed in the profile (not auto-resolved)
        let explicit_services: HashSet<String> = profile
            .services
            .keys()
            .chain(profile.infra.keys())
            .cloned()
            .collect();

        // Create an expanded profile with transitive dependencies resolved
        let step_start = std::time::Instant::now();
        let expanded = Self::expand_dependencies(profile, workspace_root).await?;
        println!(
            "      └─ Expanded {} deps in {:.2}s",
            expanded.services.len() + expanded.infra.len(),
            step_start.elapsed().as_secs_f64()
        );

        // Load all service configs in parallel
        let step_start = std::time::Instant::now();
        let configs =
            Self::load_all_configs(&expanded, workspace_root, &explicit_services).await?;
        println!(
            "      └─ Loaded {} configs in {:.2}s",
            configs.len(),
            step_start.elapsed().as_secs_f64()
        );

        // Add nodes to graph
        for (name, (config, component_type)) in configs {
            graph.nodes.insert(name.clone(), ServiceNode { name, config, component_type });
        }

        // Validate graph (no cycles, all deps exist)
        graph.validate()?;

        println!("      └─ Graph build total: {:.2}s", total_start.elapsed().as_secs_f64());

        Ok(graph)
    }

    /// Expand profile to include transitive dependencies
    async fn expand_dependencies(profile: &Profile, workspace_root: &Path) -> Result<Profile> {
        let mut expanded = profile.clone();

        // Collect all components to process
        let mut to_process: Vec<String> = Vec::new();
        to_process.extend(expanded.services.keys().cloned());
        to_process.extend(expanded.infra.keys().cloned());

        let mut processed: HashSet<String> = HashSet::new();

        // Process dependencies transitively
        while let Some(name) = to_process.pop() {
            if processed.contains(&name) {
                continue;
            }
            processed.insert(name.clone());

            // Try to load the service/infra config to find dependencies
            let deps = Self::find_dependencies(workspace_root, &name).await;

            for dep in deps {
                if !processed.contains(&dep) && !to_process.contains(&dep) {
                    // Check what type of component the dependency is
                    // Try both platform-specific and generic config paths
                    let platform =
                        std::env::var("CLOUD_PLATFORM").unwrap_or_default().to_lowercase();
                    let service_paths = [
                        workspace_root.join(format!(
                            "devdock/config/services/{dep}/devdock_config_{platform}.json"
                        )),
                        workspace_root
                            .join(format!("devdock/config/services/{dep}/devdock_config.json")),
                    ];
                    let infra_path = workspace_root.join(format!("devdock/config/infra/{dep}"));

                    let is_service = service_paths.iter().any(|p| p.exists());

                    if is_service {
                        // It's a service, add with default remote type
                        if !expanded.services.contains_key(&dep) {
                            expanded.services.insert(
                                dep.clone(),
                                ProfileServiceConfig {
                                    deployment_type: DeploymentType::Remote,
                                    env: HashMap::new(),
                                    hot_reload: None,
                                    ini_overrides: Vec::new(),
                                    script: None,
                                    service: None,
                                    depends_on_services: None,
                                    timeout: None,
                                    config: HashMap::new(),
                                },
                            );
                        }
                    } else if infra_path.exists() {
                        // It's infra, add with default local type
                        if !expanded.infra.contains_key(&dep) {
                            expanded.infra.insert(
                                dep.clone(),
                                InfraConfig {
                                    deployment_type: DeploymentType::Local,
                                    config: HashMap::new(),
                                },
                            );
                        }
                    }

                    to_process.push(dep);
                }
            }
        }

        // Add QE with remote type by default if not present (Python behavior)
        // Python: if 'services' in profile and 'qe' not in comps:
        // Only add QE if the profile already has services AND QE wasn't resolved via dependencies
        let has_services = !profile.services.is_empty();
        if has_services && !expanded.services.contains_key("qe") {
            expanded.services.insert(
                "qe".to_string(),
                ProfileServiceConfig {
                    deployment_type: DeploymentType::Remote,
                    env: HashMap::new(),
                    hot_reload: None,
                    ini_overrides: Vec::new(),
                    script: None,
                    service: None,
                    depends_on_services: None,
                    timeout: None,
                    config: HashMap::new(),
                },
            );
        }

        Ok(expanded)
    }

    /// Find dependencies for a component
    async fn find_dependencies(workspace_root: &Path, name: &str) -> Vec<String> {
        // Try platform-specific service config first, then fallback to generic
        // This matches the behavior of load_service_config
        let config_paths = [
            workspace_root.join(format!(
                "devdock/config/services/{}/devdock_config_{}.json",
                name,
                std::env::var("CLOUD_PLATFORM").unwrap_or_default().to_lowercase()
            )),
            workspace_root.join(format!("devdock/config/services/{name}/devdock_config.json")),
        ];

        for service_path in &config_paths {
            if service_path.exists() {
                if let Ok(content) = tokio::fs::read_to_string(&service_path).await {
                    if let Ok(config) = serde_json::from_str::<ServiceConfig>(&content) {
                        return config.dependencies;
                    }
                }
            }
        }

        Vec::new()
    }

    /// Load all service configurations in parallel
    async fn load_all_configs(
        profile: &Profile,
        workspace_root: &Path,
        explicit_services: &HashSet<String>,
    ) -> Result<HashMap<String, (ServiceConfig, ComponentType)>> {
        use devdock_core::types::DeploymentType;

        let mut futures = Vec::new();

        // Load service configs
        for (service_name, service_profile) in &profile.services {
            let workspace_root = workspace_root.to_path_buf();
            let service_name = service_name.clone();
            let is_local = service_profile.deployment_type == DeploymentType::Local;

            futures.push(tokio::spawn(async move {
                let mut config = Self::load_service_config(&workspace_root, &service_name).await?;
                config.set_deployment_local(is_local);
                Ok::<_, anyhow::Error>((service_name, (config, ComponentType::Service)))
            }));
        }

        // Load infra configs
        for (infra_name, infra_profile) in &profile.infra {
            let workspace_root = workspace_root.to_path_buf();
            let infra_name = infra_name.clone();
            let is_local = infra_profile.deployment_type == DeploymentType::Local;

            futures.push(tokio::spawn(async move {
                let mut config = Self::load_infra_config(&workspace_root, &infra_name).await?;
                config.set_deployment_local(is_local);
                Ok::<_, anyhow::Error>((infra_name, (config, ComponentType::Infra)))
            }));
        }

        // Wait for all to complete
        let results = futures::future::join_all(futures).await;

        // Collect results, failing fast for explicitly configured services
        let mut configs = HashMap::new();
        let mut errors = Vec::new();
        for result in results {
            match result {
                Ok(Ok((name, config_tuple))) => {
                    configs.insert(name, config_tuple);
                }
                Ok(Err(e)) => {
                    let error_msg = e.to_string();
                    let is_hook_service =
                        error_msg.contains("pre_setup") || error_msg.contains("post_setup");

                    if is_hook_service {
                        continue;
                    }

                    // Extract service name from error to check if it was explicitly configured
                    let is_explicit = explicit_services
                        .iter()
                        .any(|svc| error_msg.contains(svc));

                    if is_explicit {
                        errors.push(format!("{e:#}"));
                    } else {
                        eprintln!("Warning: Failed to load config for transitive dependency: {e:#}");
                    }
                }
                Err(e) => {
                    eprintln!("Warning: Task failed: {e}");
                }
            }
        }

        if !errors.is_empty() {
            anyhow::bail!(
                "Failed to load config for {} service(s) explicitly listed in profile:\n  - {}",
                errors.len(),
                errors.join("\n  - ")
            );
        }

        Ok(configs)
    }

    /// Load a single service config
    async fn load_service_config(
        workspace_root: &Path,
        service_name: &str,
    ) -> Result<ServiceConfig> {
        // Try platform-specific config first, then fallback to generic
        let config_paths = [
            workspace_root.join(format!(
                "devdock/config/services/{}/devdock_config_{}.json",
                service_name,
                std::env::var("CLOUD_PLATFORM").unwrap_or_default().to_lowercase()
            )),
            workspace_root
                .join(format!("devdock/config/services/{service_name}/devdock_config.json")),
        ];

        for config_path in &config_paths {
            if config_path.exists() {
                let content = tokio::fs::read_to_string(&config_path)
                    .await
                    .context(format!("Failed to read {}", config_path.display()))?;

                let mut config: ServiceConfig = serde_json::from_str(&content)
                    .context(format!("Failed to parse {}", config_path.display()))?;

                // Expand environment variables in env map
                config.env = crate::env_expansion::expand_env_vars_in_map(&config.env);

                return Ok(config);
            }
        }

        // Check if Tiltfile exists - if so, return default config
        let tiltfile_path =
            workspace_root.join(format!("devdock/config/services/{service_name}/Tiltfile"));

        if tiltfile_path.exists() {
            return Ok(ServiceConfig::default());
        }

        // Return default config for auto-resolved dependencies
        // This matches Python behavior where missing dependencies are added with defaults
        Ok(ServiceConfig::default())
    }

    /// Load a single infra config
    async fn load_infra_config(workspace_root: &Path, infra_name: &str) -> Result<ServiceConfig> {
        // Try to load config file if it exists, otherwise return minimal config
        let config_path =
            workspace_root.join(format!("devdock/config/infra/{infra_name}/devdock_config.json"));

        if config_path.exists() {
            let content = tokio::fs::read_to_string(&config_path).await?;
            let config: ServiceConfig = serde_json::from_str(&content)?;
            Ok(config)
        } else {
            // Return minimal/default config for infra without explicit config
            Ok(ServiceConfig::default())
        }
    }

    /// Validate the dependency graph
    fn validate(&self) -> Result<()> {
        // Check for missing dependencies
        // Note: We don't warn about missing dependencies because they are automatically
        // added by expand_dependencies() with default types (remote for services, local for infra).
        // If a service declares a dependency that doesn't exist in the codebase, it will
        // simply use the remote version, matching Python behavior.

        // Only check for cycles (which are actual errors)
        for name in self.nodes.keys() {
            let mut visited = HashSet::new();
            let mut stack = HashSet::new();
            if self.has_cycle(name, &mut visited, &mut stack) {
                anyhow::bail!("Dependency cycle detected involving: {name}");
            }
        }

        Ok(())
    }

    /// Check for cycles using DFS
    fn has_cycle(
        &self,
        name: &str,
        visited: &mut HashSet<String>,
        stack: &mut HashSet<String>,
    ) -> bool {
        if stack.contains(name) {
            return true;
        }

        if visited.contains(name) {
            return false;
        }

        visited.insert(name.to_string());
        stack.insert(name.to_string());

        if let Some(node) = self.nodes.get(name) {
            for dep in &node.config.dependencies {
                if self.has_cycle(dep, visited, stack) {
                    return true;
                }
            }
        }

        stack.remove(name);
        false
    }

    /// Get all nodes
    pub fn nodes(&self) -> impl Iterator<Item = &ServiceNode> {
        self.nodes.values()
    }

    /// Get a specific node
    pub fn get(&self, name: &str) -> Option<&ServiceNode> {
        self.nodes.get(name)
    }

    /// Get number of nodes
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Get all service names
    pub fn service_names(&self) -> Vec<String> {
        self.nodes
            .values()
            .filter(|n| n.component_type == ComponentType::Service)
            .map(|n| n.name.clone())
            .collect()
    }
}
