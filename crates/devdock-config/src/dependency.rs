//! Dependency resolution for services and infrastructure.

use devdock_core::{profile::Profile, types::ComponentName, Error, Result};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

/// Resolves dependencies between components.
pub struct DependencyResolver {
    /// Path to monorepo root (for loading service metadata).
    monorepo_root: Option<PathBuf>,
}

impl DependencyResolver {
    /// Create a new dependency resolver.
    pub fn new() -> Self {
        Self {
            monorepo_root: None,
        }
    }

    /// Create a resolver with a monorepo root path.
    pub fn with_monorepo_root(root: PathBuf) -> Self {
        Self {
            monorepo_root: Some(root),
        }
    }

    /// Resolve all dependencies in a profile and return ordered components.
    pub fn resolve(&self, profile: &Profile) -> Result<Vec<ComponentName>> {
        let mut graph = self.build_dependency_graph(profile)?;
        self.topological_sort(&mut graph)
    }

    /// Build a dependency graph from the profile.
    fn build_dependency_graph(
        &self,
        profile: &Profile,
    ) -> Result<HashMap<ComponentName, Vec<ComponentName>>> {
        let mut graph: HashMap<ComponentName, Vec<ComponentName>> = HashMap::new();

        // Add all components to graph
        for name in profile.all_components() {
            graph.entry(name.clone()).or_insert_with(Vec::new);
        }

        // Load service dependencies from metadata files
        if let Some(ref root) = self.monorepo_root {
            for (service_name, _config) in &profile.services {
                let deps = self.load_service_dependencies(root, service_name)?;
                for dep in deps {
                    if profile.has_component(&dep) {
                        graph.entry(service_name.clone()).or_default().push(dep);
                    }
                }
            }
        }

        // Add hardcoded infrastructure dependencies
        self.add_infra_dependencies(&mut graph, profile);

        Ok(graph)
    }

    /// Load service dependencies from devdock_config.json.
    fn load_service_dependencies(
        &self,
        monorepo_root: &Path,
        service_name: &str,
    ) -> Result<Vec<ComponentName>> {
        let config_path = monorepo_root
            .join("devdock/config/services")
            .join(service_name)
            .join("devdock_config.json");

        if !config_path.exists() {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(&config_path)?;
        let json: serde_json::Value = serde_json::from_str(&content)?;

        if let Some(deps) = json.get("dependencies").and_then(|d| d.as_array()) {
            let dep_names: Vec<ComponentName> = deps
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            Ok(dep_names)
        } else {
            Ok(Vec::new())
        }
    }

    /// Add known infrastructure dependencies.
    fn add_infra_dependencies(
        &self,
        graph: &mut HashMap<ComponentName, Vec<ComponentName>>,
        profile: &Profile,
    ) {
        // opensearch remote depends on bastion
        if profile.infra.contains_key("opensearch") {
            if let Some(opensearch_config) = profile.infra.get("opensearch") {
                if matches!(
                    opensearch_config.deployment_type,
                    devdock_core::types::DeploymentType::Remote
                ) {
                    if profile.infra.contains_key("bastion") {
                        graph
                            .entry("opensearch".to_string())
                            .or_default()
                            .push("bastion".to_string());
                    }
                }
            }
        }

        // sql remote depends on bastion
        if profile.infra.contains_key("sql") {
            if let Some(sql_config) = profile.infra.get("sql") {
                if matches!(
                    sql_config.deployment_type,
                    devdock_core::types::DeploymentType::Remote
                ) {
                    if profile.infra.contains_key("bastion") {
                        graph
                            .entry("sql".to_string())
                            .or_default()
                            .push("bastion".to_string());
                    }
                }
            }
        }
    }

    /// Perform topological sort to get dependency-ordered components.
    fn topological_sort(
        &self,
        graph: &HashMap<ComponentName, Vec<ComponentName>>,
    ) -> Result<Vec<ComponentName>> {
        let mut in_degree: HashMap<ComponentName, usize> = HashMap::new();
        let mut result = Vec::new();

        // Calculate in-degrees
        for node in graph.keys() {
            in_degree.entry(node.clone()).or_insert(0);
        }
        for edges in graph.values() {
            for edge in edges {
                *in_degree.entry(edge.clone()).or_insert(0) += 1;
            }
        }

        // Find nodes with no dependencies
        let mut queue: VecDeque<ComponentName> = in_degree
            .iter()
            .filter(|(_, &degree)| degree == 0)
            .map(|(name, _)| name.clone())
            .collect();

        // Process nodes
        while let Some(node) = queue.pop_front() {
            result.push(node.clone());

            // Reduce in-degree for neighbors
            if let Some(edges) = graph.get(&node) {
                for edge in edges {
                    if let Some(degree) = in_degree.get_mut(edge) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(edge.clone());
                        }
                    }
                }
            }
        }

        // Check for cycles
        if result.len() != graph.len() {
            return Err(Error::DependencyCycle(
                "Circular dependency detected in profile".to_string(),
            ));
        }

        Ok(result)
    }
}

impl Default for DependencyResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devdock_core::profile::{InfraConfig, ServiceConfig};
    use devdock_core::types::{DeploymentType, Platform};
    use std::collections::HashMap;

    #[test]
    fn test_simple_dependency_resolution() {
        let mut infra = HashMap::new();
        infra.insert(
            "sql".to_string(),
            InfraConfig {
                deployment_type: DeploymentType::Local,
                config: HashMap::new(),
            },
        );

        let mut services = HashMap::new();
        services.insert(
            "qe".to_string(),
            ServiceConfig {
                deployment_type: DeploymentType::Local,
                env: HashMap::new(),
                hot_reload: None,
                config: HashMap::new(),
            },
        );

        let profile = Profile {
            project: "test".to_string(),
            platform: Platform::Google,
            infra,
            services,
            pipelines: HashMap::new(),
            test_suite: HashMap::new(),
            env: HashMap::new(),
            multi_project: None,
        };

        let resolver = DependencyResolver::new();
        let resolved = resolver.resolve(&profile).unwrap();

        // sql should come before qe (even without explicit deps, infra first)
        assert!(resolved.contains(&"sql".to_string()));
        assert!(resolved.contains(&"qe".to_string()));
    }
}

