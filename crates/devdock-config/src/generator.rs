//! Main configuration generator orchestrator.

use crate::bazel::BazelQueryEngine;
use crate::dependency_graph::DependencyGraph;
use crate::gcs_storage::DynamicIniLoader;
use crate::ini_generator::IniGenerator;
use crate::ini_parser::ParsedIniConfig;
use crate::platform::{create_platform, Platform};
use crate::profile_generator::TempProfileGenerator;
use anyhow::{Context, Result};
use devdock_core::profile::Profile;
use std::collections::HashMap;
use std::path::PathBuf;

/// Main configuration generator
pub struct ConfigGenerator {
    workspace_root: PathBuf,
    fetch_dynamic_config: bool,
}

impl ConfigGenerator {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            workspace_root,
            fetch_dynamic_config: true,
        }
    }

    /// Set whether to fetch dynamic config from GCS
    pub fn with_dynamic_config(mut self, enabled: bool) -> Self {
        self.fetch_dynamic_config = enabled;
        self
    }

    /// Generate all configurations in parallel
    pub async fn generate(
        &self,
        profile: &Profile,
        active_services: Option<&str>,
        profile_path: Option<&str>,
    ) -> Result<()> {
        println!("⏩ Generating configurations (Rust - parallel)...");
        let start = std::time::Instant::now();

        // Ensure output directory exists
        let runtime_gen = self.workspace_root.join("devdock/runtime_generated");
        tokio::fs::create_dir_all(&runtime_gen).await?;

        // 1. Load all service configs in parallel with dynamic INI
        println!("⏩ Loading service configs and dynamic INI...");
        let (graph_result, dynamic_ini) = tokio::join!(
            DependencyGraph::build(profile, &self.workspace_root),
            self.load_dynamic_ini(&profile.project),
        );

        let graph = graph_result?;
        println!(
            "✅ Loaded {} service configs in {:?}",
            graph.node_count(),
            start.elapsed()
        );

        // 2. Create platform object with dynamic config for zone/region
        println!("⏩ Initializing platform...");
        let platform = create_platform(
            &profile.platform.to_string(),
            profile.project.clone(),
            dynamic_ini.as_ref(),
        )
            .await
            .context("Failed to create platform")?;
        println!("✅ Platform initialized in {:?}", start.elapsed());

        // 3. Platform authentication (optional, can be slow)
        println!("⏩ Authenticating with platform...");
        if let Err(e) = platform.authenticate().await {
            eprintln!("Warning: Platform authentication failed: {}", e);
        } else {
            println!("✅ Platform authenticated in {:?}", start.elapsed());
        }

        // 4. Generate all configs in parallel
        println!("⏩ Generating configs...");
        let (devdock_ini, temp_profile, _watch_result) = tokio::join!(
            self.generate_devdock_ini(profile, platform.as_ref(), &graph, dynamic_ini.as_ref()),
            self.generate_temp_profile(profile, platform.as_ref(), &graph, active_services, profile_path, dynamic_ini.clone()),
            self.generate_watch_files(&graph),
        );

        let devdock_ini = devdock_ini?;
        let temp_profile = temp_profile?;

        println!("✅ Generated all configs in {:?}", start.elapsed());

        // 5. Write all files in parallel
        println!("⏩ Writing config files...");
        tokio::try_join!(
            tokio::fs::write(
                self.workspace_root.join("devdock/devdock.ini"),
                &devdock_ini
            ),
            tokio::fs::write(self.workspace_root.join("config/devdock.ini"), &devdock_ini),
            tokio::fs::write(
                self.workspace_root.join("config/run_local.ini"),
                &devdock_ini
            ),
            tokio::fs::write(runtime_gen.join("temp_profile.yaml"), &temp_profile),
        )?;

        println!(
            "✅ Config generation completed in {:?}",
            start.elapsed()
        );
        Ok(())
    }

    /// Load dynamic INI configuration from GCS
    async fn load_dynamic_ini(&self, project: &str) -> Option<ParsedIniConfig> {
        if !self.fetch_dynamic_config {
            return None;
        }

        match DynamicIniLoader::new(project, self.workspace_root.clone()) {
            Ok(loader) => {
                match loader.fetch_dynamic_ini().await {
                    Ok(config) => {
                        if config.is_some() {
                            println!("✅ Fetched dynamic.ini from GCS");
                        }
                        config
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to fetch dynamic.ini: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                eprintln!("Warning: Failed to create INI loader: {}", e);
                None
            }
        }
    }

    /// Generate devdock.ini configuration
    async fn generate_devdock_ini(
        &self,
        profile: &Profile,
        platform: &dyn Platform,
        graph: &DependencyGraph,
        dynamic_ini: Option<&ParsedIniConfig>,
    ) -> Result<String> {
        use crate::ini_generator::IniGeneratorContext;

        // Create context from profile
        let context = IniGeneratorContext::from_profile(profile);

        // Create generator with context, optionally with base config from dynamic INI
        let mut generator = if let Some(base) = dynamic_ini {
            IniGenerator::with_base_config_and_context(base, context)
        } else {
            IniGenerator::with_context(context)
        };

        // Apply platform config
        generator.apply_platform(platform);

        // Apply all services and dependencies
        for node in graph.nodes() {
            generator.apply_service(&node.name, &node.config);
            generator.apply_dependency(node);
        }

        Ok(generator.generate())
    }

    /// Generate temp_profile.yaml configuration
    async fn generate_temp_profile(
        &self,
        profile: &Profile,
        platform: &dyn Platform,
        graph: &DependencyGraph,
        active_services: Option<&str>,
        profile_path: Option<&str>,
        dynamic_ini: Option<ParsedIniConfig>,
    ) -> Result<String> {
        let mut generator = TempProfileGenerator::new(&self.workspace_root, profile_path);

        // If we have dynamic.ini, pass it to the generator for namespace resolution
        if let Some(ini) = dynamic_ini {
            generator = generator.with_dynamic_ini(ini);
        }

        // Apply profile-level settings (vault_key, pipelines, test_suite, env)
        generator.apply_profile_settings(profile);

        // Apply platform config
        generator.apply_platform(platform);

        // Apply all services and infra with profile data
        generator.apply_services(graph, profile, active_services);

        generator.generate()
    }

    /// Generate watch files for hot reload
    async fn generate_watch_files(&self, graph: &DependencyGraph) -> Result<()> {
        // Extract services with bazel targets
        let services_with_targets: HashMap<String, String> = graph
            .nodes()
            .filter_map(|n| {
                n.config
                    .bazel_target
                    .as_ref()
                    .map(|t| (n.name.clone(), t.clone()))
            })
            .collect();

        if services_with_targets.is_empty() {
            return Ok(());
        }

        let engine = BazelQueryEngine::new(self.workspace_root.clone());
        engine.generate_watch_files(services_with_targets).await
    }
}

