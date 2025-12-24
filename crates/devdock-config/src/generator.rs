//! Main configuration generator orchestrator.

use crate::bazel::BazelQueryEngine;
use crate::dependency_graph::DependencyGraph;
use crate::gcs_storage::DynamicIniLoader;
use crate::ini_generator::IniGenerator;
use crate::ini_parser::ParsedIniConfig;
use crate::platform::{create_platform, Platform};
use crate::profile_generator::TempProfileGenerator;
use crate::watch_patterns::{WatchPatternGenerator, ServiceType as WatchServiceType};
use anyhow::{Context, Result};
use devdock_core::profile::Profile;
use std::collections::HashMap;
use std::path::PathBuf;

/// Strategy for generating watch files
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchStrategy {
    /// Use static glob patterns (instant, no bazel) - DEFAULT
    Static,
    /// Use bazel cquery (accurate but slow)
    Cquery,
    /// Static first, then background cquery to refine
    Hybrid,
    /// Skip watch file generation entirely
    Skip,
}

impl Default for WatchStrategy {
    fn default() -> Self {
        WatchStrategy::Static
    }
}

/// Main configuration generator
pub struct ConfigGenerator {
    workspace_root: PathBuf,
    fetch_dynamic_config: bool,
    watch_strategy: WatchStrategy,
}

impl ConfigGenerator {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            workspace_root,
            fetch_dynamic_config: true,
            watch_strategy: WatchStrategy::default(),
        }
    }

    /// Set whether to fetch dynamic config from GCS
    pub fn with_dynamic_config(mut self, enabled: bool) -> Self {
        self.fetch_dynamic_config = enabled;
        self
    }

    /// Set the watch file generation strategy
    ///
    /// - `Static`: Use pre-computed glob patterns (instant, default)
    /// - `Cquery`: Use bazel cquery (accurate but slow)
    /// - `Hybrid`: Static first, background cquery to refine
    /// - `Skip`: Don't generate watch files
    pub fn with_watch_strategy(mut self, strategy: WatchStrategy) -> Self {
        self.watch_strategy = strategy;
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
        let total_start = std::time::Instant::now();

        // Ensure output directory exists
        let runtime_gen = self.workspace_root.join("devdock/runtime_generated");
        tokio::fs::create_dir_all(&runtime_gen).await?;

        // 1. Load all service configs in parallel with dynamic INI
        println!("⏩ Loading service configs and dynamic INI...");
        let step_start = std::time::Instant::now();
        let (graph_result, dynamic_ini) = tokio::join!(
            DependencyGraph::build(profile, &self.workspace_root),
            self.load_dynamic_ini(&profile.project),
        );

        let graph = graph_result?;
        println!(
            "✅ Loaded {} service configs in {:.2}s (total: {:.2}s)",
            graph.node_count(),
            step_start.elapsed().as_secs_f64(),
            total_start.elapsed().as_secs_f64()
        );

        // 2. Create platform object with dynamic config for zone/region
        println!("⏩ Initializing platform...");
        let step_start = std::time::Instant::now();
        let platform = create_platform(
            &profile.platform.to_string(),
            profile.project.clone(),
            dynamic_ini.as_ref(),
        )
            .await
            .context("Failed to create platform")?;
        println!("✅ Platform initialized in {:.2}s (total: {:.2}s)", 
            step_start.elapsed().as_secs_f64(),
            total_start.elapsed().as_secs_f64()
        );

        // 3. Platform authentication (optional, can be slow)
        println!("⏩ Authenticating with platform...");
        let step_start = std::time::Instant::now();
        if let Err(e) = platform.authenticate().await {
            eprintln!("Warning: Platform authentication failed: {}", e);
        } else {
            println!("✅ Platform authenticated in {:.2}s (total: {:.2}s)", 
                step_start.elapsed().as_secs_f64(),
                total_start.elapsed().as_secs_f64()
            );
        }

        // 4. Generate all configs in parallel with detailed timing
        println!("⏩ Generating configs (3 parallel tasks)...");
        let step_start = std::time::Instant::now();
        
        // Use separate timers for each task
        let ini_start = std::time::Instant::now();
        let profile_start = std::time::Instant::now();
        let watch_start = std::time::Instant::now();
        
        let (devdock_ini, temp_profile, watch_result) = tokio::join!(
            async {
                let result = self.generate_devdock_ini(profile, platform.as_ref(), &graph, dynamic_ini.as_ref()).await;
                println!("   └─ devdock.ini generated in {:.2}s", ini_start.elapsed().as_secs_f64());
                result
            },
            async {
                let result = self.generate_temp_profile(profile, platform.as_ref(), &graph, active_services, profile_path, dynamic_ini.clone()).await;
                println!("   └─ temp_profile.yaml generated in {:.2}s", profile_start.elapsed().as_secs_f64());
                result
            },
            async {
                let result = self.generate_watch_files(&graph).await;
                println!("   └─ watch files generated in {:.2}s", watch_start.elapsed().as_secs_f64());
                result
            },
        );

        let devdock_ini = devdock_ini?;
        let temp_profile = temp_profile?;
        if let Err(e) = watch_result {
            eprintln!("   └─ Warning: watch file generation failed: {}", e);
        }

        println!("✅ Generated all configs in {:.2}s (total: {:.2}s)", 
            step_start.elapsed().as_secs_f64(),
            total_start.elapsed().as_secs_f64()
        );

        // 5. Write all files in parallel
        println!("⏩ Writing config files...");
        let step_start = std::time::Instant::now();
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
            "✅ Config files written in {:.2}s (TOTAL: {:.2}s)",
            step_start.elapsed().as_secs_f64(),
            total_start.elapsed().as_secs_f64()
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
        match self.watch_strategy {
            WatchStrategy::Skip => {
                println!("      └─ Watch file generation skipped");
                return Ok(());
            }
            WatchStrategy::Static => {
                return self.generate_watch_files_static(graph).await;
            }
            WatchStrategy::Cquery => {
                return self.generate_watch_files_cquery(graph).await;
            }
            WatchStrategy::Hybrid => {
                // Generate static first (fast)
                self.generate_watch_files_static(graph).await?;
                // TODO: Spawn background task to refine with cquery
                println!("      └─ Background cquery refinement not yet implemented");
                return Ok(());
            }
        }
    }

    /// Generate watch files using static glob patterns (instant, no bazel)
    async fn generate_watch_files_static(&self, graph: &DependencyGraph) -> Result<()> {
        use crate::service_config::ServiceType as ConfigServiceType;

        let generator = WatchPatternGenerator::new(self.workspace_root.clone())?;

        // Collect services with their types and targets
        let services: Vec<(String, WatchServiceType, Option<String>)> = graph
            .nodes()
            .map(|n| {
                // Convert ServiceConfig::ServiceType to WatchServiceType
                let svc_type = match n.config.service_type {
                    ConfigServiceType::Go => WatchServiceType::Go,
                    ConfigServiceType::Python => WatchServiceType::Python,
                    ConfigServiceType::Java => WatchServiceType::Java,
                    _ => WatchServiceType::Unknown,
                };
                let target = n.config.bazel_target.clone();
                (n.name.clone(), svc_type, target)
            })
            .collect();

        if services.is_empty() {
            println!("      └─ No services to generate watch files for");
            return Ok(());
        }

        println!("      └─ Generating static watch patterns for {} service(s)", services.len());
        let start = std::time::Instant::now();

        generator.generate_all_watch_files(&services).await?;

        println!("      └─ Static watch patterns generated in {:.2}ms", 
            start.elapsed().as_secs_f64() * 1000.0);
        Ok(())
    }

    /// Generate watch files using bazel cquery (accurate but slow)
    async fn generate_watch_files_cquery(&self, graph: &DependencyGraph) -> Result<()> {
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

