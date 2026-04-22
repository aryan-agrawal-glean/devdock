//! Main configuration generator orchestrator.

use crate::bazel::BazelQueryEngine;
use crate::dependency_graph::DependencyGraph;
use crate::gcs_storage::DynamicIniLoader;
use crate::ini_generator::IniGenerator;
use crate::ini_parser::ParsedIniConfig;
use crate::platform::{create_platform, Platform};
use crate::profile_generator::TempProfileGenerator;
use crate::watch_patterns::{ServiceType as WatchServiceType, WatchPatternGenerator};
use anyhow::{Context, Result};
use devdock_core::profile::Profile;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::process::Command;
use tracing::debug;

/// Strategy for generating watch files
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WatchStrategy {
    /// Use static glob patterns (instant, no bazel) - DEFAULT
    #[default]
    Static,
    /// Use bazel cquery (accurate but slow)
    Cquery,
    /// Static first, then background cquery to refine
    Hybrid,
    /// Skip watch file generation entirely
    Skip,
}

/// Main configuration generator
pub struct ConfigGenerator {
    workspace_root: PathBuf,
    fetch_dynamic_config: bool,
    watch_strategy: WatchStrategy,
    skip_platform_init: bool,
    skip_file_writes: bool,
}

impl ConfigGenerator {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            workspace_root,
            fetch_dynamic_config: true,
            watch_strategy: WatchStrategy::default(),
            skip_platform_init: false,
            skip_file_writes: false,
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

    /// Skip platform initialization (for benchmarking/CI)
    pub fn with_skip_platform_init(mut self, skip: bool) -> Self {
        self.skip_platform_init = skip;
        self
    }

    /// Skip writing files to disk (for benchmarking/CI)
    pub fn with_skip_file_writes(mut self, skip: bool) -> Self {
        self.skip_file_writes = skip;
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
            self.load_dynamic_ini(profile),
        );

        let graph = graph_result?;
        println!(
            "✅ Loaded {} service configs in {:.2}s (total: {:.2}s)",
            graph.node_count(),
            step_start.elapsed().as_secs_f64(),
            total_start.elapsed().as_secs_f64()
        );

        // 2. Create platform object with dynamic config for zone/region
        let platform = if self.skip_platform_init {
            // Use mock platform for benchmarking (no external API calls)
            println!("⏩ Using mock platform (benchmark mode)...");
            use crate::platform::create_mock_platform;
            create_mock_platform(profile.project.clone())
        } else {
            println!("⏩ Initializing platform...");
            let step_start = std::time::Instant::now();
            let platform = create_platform(
                &profile.platform.to_string(),
                profile.project.clone(),
                dynamic_ini.as_ref(),
            )
            .await
            .context(format!(
                "Failed to create platform '{}' for project '{}'. \
                    This usually indicates authentication issues with your cloud provider. \
                    See error details above.",
                profile.platform, profile.project
            ))?;
            println!(
                "✅ Platform initialized in {:.2}s (total: {:.2}s)",
                step_start.elapsed().as_secs_f64(),
                total_start.elapsed().as_secs_f64()
            );

            // 3. Platform authentication (optional, can be slow)
            println!("⏩ Authenticating with platform...");
            let step_start = std::time::Instant::now();
            if let Err(e) = platform.authenticate().await {
                eprintln!("Warning: Platform authentication failed: {e}");
            } else {
                println!(
                    "✅ Platform authenticated in {:.2}s (total: {:.2}s)",
                    step_start.elapsed().as_secs_f64(),
                    total_start.elapsed().as_secs_f64()
                );
            }

            platform
        };

        // 4. Generate all configs in parallel with detailed timing
        println!("⏩ Generating configs (3 parallel tasks)...");
        let step_start = std::time::Instant::now();

        // Use separate timers for each task
        let ini_start = std::time::Instant::now();
        let profile_start = std::time::Instant::now();
        let watch_start = std::time::Instant::now();

        let (devdock_ini, temp_profile, watch_result) = tokio::join!(
            async {
                let result = self
                    .generate_devdock_ini(profile, platform.as_ref(), &graph, dynamic_ini.as_ref())
                    .await;
                println!(
                    "   └─ devdock.ini generated in {:.2}s",
                    ini_start.elapsed().as_secs_f64()
                );
                result
            },
            async {
                let result = self
                    .generate_temp_profile(
                        profile,
                        platform.as_ref(),
                        &graph,
                        active_services,
                        profile_path,
                        dynamic_ini.clone(),
                    )
                    .await;
                println!(
                    "   └─ temp_profile.yaml generated in {:.2}s",
                    profile_start.elapsed().as_secs_f64()
                );
                result
            },
            async {
                let result = self.generate_watch_files(&graph).await;
                println!(
                    "   └─ watch files generated in {:.2}s",
                    watch_start.elapsed().as_secs_f64()
                );
                result
            },
        );

        let devdock_ini = devdock_ini?;
        let temp_profile = temp_profile?;
        if let Err(e) = watch_result {
            eprintln!("   └─ Warning: watch file generation failed: {e}");
        }

        println!(
            "✅ Generated all configs in {:.2}s (total: {:.2}s)",
            step_start.elapsed().as_secs_f64(),
            total_start.elapsed().as_secs_f64()
        );

        // 5. Write all files in parallel (skip in benchmark mode)
        if !self.skip_file_writes {
            println!("⏩ Writing config files...");
            let step_start = std::time::Instant::now();
            tokio::try_join!(
                tokio::fs::write(self.workspace_root.join("devdock/devdock.ini"), &devdock_ini),
                tokio::fs::write(self.workspace_root.join("config/devdock.ini"), &devdock_ini),
                tokio::fs::write(self.workspace_root.join("config/run_local.ini"), &devdock_ini),
                tokio::fs::write(runtime_gen.join("temp_profile.yaml"), &temp_profile),
            )?;

            println!(
                "✅ Config files written in {:.2}s (TOTAL: {:.2}s)",
                step_start.elapsed().as_secs_f64(),
                total_start.elapsed().as_secs_f64()
            );
        } else {
            println!(
                "✅ Config generation complete (TOTAL: {:.2}s)",
                total_start.elapsed().as_secs_f64()
            );
        }

        // 6. Handle multi-project (glean-apps-canary) if present
        if let Some(ref multi_config) = profile.multi_project {
            self.generate_multi_project_configs(
                profile,
                multi_config,
                &devdock_ini,
                dynamic_ini.as_ref(),
            )
            .await?;
        }

        Ok(())
    }

    /// Generate multi-project configurations (scio_apps.ini and nested temp_profile)
    async fn generate_multi_project_configs(
        &self,
        primary_profile: &Profile,
        multi_config: &devdock_core::profile::MultiProjectConfig,
        _primary_ini: &str,
        dynamic_ini: Option<&ParsedIniConfig>,
    ) -> Result<()> {
        println!("⏩ Generating multi-project (glean-apps-canary) configs...");
        let step_start = std::time::Instant::now();

        // The multi-project is always glean-apps-canary for now
        let multi_project_name = "glean-apps-canary";

        // 1. Create platform for multi-project
        let multi_platform = crate::platform::create_platform_with_multi_project(
            "google", // Multi-project is always GCP for now
            multi_project_name.to_string(),
            dynamic_ini,
            true, // multi_project flag
        )
        .await?;

        // 2. Build dependency graph for multi-project services/infra
        let mut multi_profile = Profile {
            project: multi_project_name.to_string(),
            platform: primary_profile.platform,
            infra: multi_config.infra.clone(),
            services: multi_config.services.clone(),
            pipelines: HashMap::new(),
            test_suite: HashMap::new(),
            env: HashMap::new(),
            ini_overrides: Vec::new(),
            multi_project: None,
        };

        let multi_graph = DependencyGraph::build(&multi_profile, &self.workspace_root).await?;

        // 3. Auto-inject SQL if not present (matches Python behavior)
        let has_sql = multi_config.infra.contains_key("sql");
        if has_sql {
            // Generate scio_apps.ini without auto-injecting SQL
            self.generate_multi_project_ini(
                &multi_profile,
                multi_platform.as_ref(),
                &multi_graph,
                dynamic_ini,
            )
            .await?;
        } else {
            use devdock_core::types::DeploymentType;
            use devdock_core::InfraConfig;

            multi_profile.infra.insert(
                "sql".to_string(),
                InfraConfig { deployment_type: DeploymentType::Remote, config: HashMap::new() },
            );

            // Rebuild graph with SQL included
            let multi_graph = DependencyGraph::build(&multi_profile, &self.workspace_root).await?;

            // Generate scio_apps.ini with SQL
            self.generate_multi_project_ini(
                &multi_profile,
                multi_platform.as_ref(),
                &multi_graph,
                dynamic_ini,
            )
            .await?;
        }

        println!(
            "✅ Multi-project configs generated in {:.2}s",
            step_start.elapsed().as_secs_f64()
        );
        Ok(())
    }

    /// Generate scio_apps.ini for multi-project
    async fn generate_multi_project_ini(
        &self,
        profile: &Profile,
        platform: &dyn Platform,
        graph: &DependencyGraph,
        dynamic_ini: Option<&ParsedIniConfig>,
    ) -> Result<()> {
        use crate::ini_generator::IniGeneratorContext;
        use crate::platform::TempProfile;

        // Extract account_id from platform (for AWS)
        let mut temp_profile_for_account_id = TempProfile::default();
        platform.update_temp_profile(&mut temp_profile_for_account_id);

        // Extract opensearch namespace from multi-project profile if present
        let opensearch_namespace = profile.infra.get("opensearch").and_then(|opensearch| {
            opensearch.config.get("namespace").and_then(|ns| ns.as_str().map(String::from))
        });

        // Create context with multi_project=true
        let mut context = IniGeneratorContext {
            multi_project: true,
            has_quality_pipelines: false,
            project: profile.project.clone(),
            platform: profile.platform.to_string(),
            account_id: temp_profile_for_account_id.account_id,
            sql_metadata: None,
            opensearch_namespace,
        };

        // Pre-fetch SQL metadata if SQL is remote
        if let Some(metadata) = self
            .fetch_sql_metadata_if_needed(profile, platform, dynamic_ini, "multi-project INI")
            .await
        {
            context = context.with_sql_metadata(metadata);
        }

        // Create generator with context
        let mut generator = if let Some(base) = dynamic_ini {
            IniGenerator::with_base_config_and_context(base, context)
        } else {
            IniGenerator::with_context(context)
        };

        // Apply platform config
        generator.apply_platform(platform);

        // Apply all services and dependencies
        // NOTE: env vars (IS_DEVDOCK, DEVDOCK_ID, etc.) are NOT injected here because
        // the INI generator ignores env — they flow through profile_generator → temp_profile.yaml
        for node in graph.nodes() {
            generator.apply_service(&node.name, &node.config);
            generator.apply_dependency(node);
        }

        // Apply service-level ini_overrides from the profile
        for service_config in profile.services.values() {
            if !service_config.ini_overrides.is_empty() {
                generator.apply_profile_overrides(&service_config.ini_overrides);
            }
        }

        let scio_apps_ini = generator.generate();

        // Write scio_apps.ini
        let scio_apps_path = self.workspace_root.join("devdock/scio_apps.ini");
        tokio::fs::write(&scio_apps_path, &scio_apps_ini).await?;
        println!("   └─ scio_apps.ini written");

        Ok(())
    }

    /// Load dynamic INI configuration from GCS or S3 (platform-specific)
    async fn load_dynamic_ini(&self, profile: &Profile) -> Option<ParsedIniConfig> {
        use crate::s3_storage::S3DynamicIniLoader;

        if !self.fetch_dynamic_config {
            return None;
        }

        // Determine which storage backend to use based on platform
        let platform_str = profile.platform.to_string();

        match platform_str.as_str() {
            "aws" => {
                // For AWS, we need to get the AWS account ID first
                // Try from profile project (might be account ID) or fetch via AWS CLI
                let account_id = if profile.project.chars().all(|c| c.is_ascii_digit())
                    && profile.project.len() == 12
                {
                    // Already an account ID
                    profile.project.clone()
                } else {
                    // Fetch account ID from AWS CLI
                    match Command::new("aws")
                        .args([
                            "sts",
                            "get-caller-identity",
                            "--query",
                            "Account",
                            "--output",
                            "text",
                        ])
                        .output()
                        .await
                    {
                        Ok(output) if output.status.success() => {
                            String::from_utf8_lossy(&output.stdout).trim().to_string()
                        }
                        _ => {
                            eprintln!("Warning: Failed to get AWS account ID for S3 bucket");
                            return None;
                        }
                    }
                };

                let region = std::env::var("AWS_REGION")
                    .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
                    .ok();

                match S3DynamicIniLoader::new(&account_id, self.workspace_root.clone(), region) {
                    Ok(loader) => match loader.fetch_dynamic_ini().await {
                        Ok(config) => {
                            if config.is_some() {
                                println!("✅ Fetched dynamic.ini from S3");
                            }
                            config
                        }
                        Err(e) => {
                            eprintln!("Warning: Failed to fetch dynamic.ini from S3: {e}");
                            None
                        }
                    },
                    Err(e) => {
                        eprintln!("Warning: Failed to create S3 INI loader: {e}");
                        None
                    }
                }
            }
            "google" | "gcp" => {
                // For Google Cloud, use GCS
                match DynamicIniLoader::new(&profile.project, self.workspace_root.clone()).await {
                    Ok(loader) => match loader.fetch_dynamic_ini().await {
                        Ok(config) => {
                            if config.is_some() {
                                println!("✅ Fetched dynamic.ini from GCS");
                            }
                            config
                        }
                        Err(e) => {
                            eprintln!("Warning: Failed to fetch dynamic.ini from GCS: {e}");
                            None
                        }
                    },
                    Err(e) => {
                        eprintln!("Warning: Failed to create GCS INI loader: {e}");
                        None
                    }
                }
            }
            _ => {
                // For other platforms (azure, unknown), try GCS as fallback
                match DynamicIniLoader::new(&profile.project, self.workspace_root.clone()).await {
                    Ok(loader) => match loader.fetch_dynamic_ini().await {
                        Ok(config) => config,
                        Err(e) => {
                            debug!("Failed to fetch dynamic.ini: {e}");
                            None
                        }
                    },
                    Err(e) => {
                        debug!("Failed to create INI loader: {e}");
                        None
                    }
                }
            }
        }
    }

    /// Load a project-specific dynamic.ini from GCS.
    ///
    /// Used by the multi-project (glean-apps-canary) path so it can read that
    /// project's own SQL instance names rather than the primary profile's.
    /// GCP-only for now because multi-project currently hardcodes `google`.
    async fn load_dynamic_ini_for_project(&self, project: &str) -> Option<ParsedIniConfig> {
        if !self.fetch_dynamic_config {
            return None;
        }

        match DynamicIniLoader::new(project, self.workspace_root.clone()).await {
            Ok(loader) => match loader.fetch_dynamic_ini().await {
                Ok(config) => {
                    if config.is_some() {
                        println!("✅ Fetched dynamic.ini from GCS for project '{}'", project);
                    }
                    config
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to fetch dynamic.ini for project '{}' from GCS: {}",
                        project, e
                    );
                    None
                }
            },
            Err(e) => {
                eprintln!(
                    "Warning: Failed to create GCS INI loader for project '{}': {}",
                    project, e
                );
                None
            }
        }
    }

    /// Fetch SQL metadata if SQL is remote and dynamic_ini is present
    async fn fetch_sql_metadata_if_needed(
        &self,
        profile: &Profile,
        platform: &dyn Platform,
        dynamic_ini: Option<&ParsedIniConfig>,
        context_name: &str,
    ) -> Option<crate::sql::SqlRemoteMetadata> {
        if let Some(sql_config) = profile.infra.get("sql") {
            if sql_config.deployment_type == devdock_core::types::DeploymentType::Remote {
                if let Some(ini) = dynamic_ini {
                    match crate::sql::SqlUtils::fetch_remote_metadata(
                        &profile.project,
                        &profile.platform.to_string(),
                        platform.zone(),
                        ini,
                    )
                    .await
                    {
                        Ok(metadata) => return Some(metadata),
                        Err(e) => {
                            eprintln!(
                                "Warning: Failed to fetch SQL metadata for {}: {}",
                                context_name, e
                            );
                        }
                    }
                }
            }
        }
        None
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
        use crate::platform::TempProfile;

        // Create context from profile
        let mut context = IniGeneratorContext::from_profile(profile);

        // Extract account_id from platform (for AWS)
        let mut temp_profile_for_account_id = TempProfile::default();
        platform.update_temp_profile(&mut temp_profile_for_account_id);
        context.account_id = temp_profile_for_account_id.account_id;

        // Pre-fetch SQL metadata if SQL is remote
        if let Some(metadata) =
            self.fetch_sql_metadata_if_needed(profile, platform, dynamic_ini, "INI").await
        {
            context = context.with_sql_metadata(metadata);
        }

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

        // Apply service-level ini_overrides from the profile
        // These override the ini_overrides from devdock_config.json
        for service_config in profile.services.values() {
            if !service_config.ini_overrides.is_empty() {
                generator.apply_profile_overrides(&service_config.ini_overrides);
            }
        }

        // Apply profile-level ini_overrides (these take precedence over everything)
        generator.apply_profile_overrides(&profile.ini_overrides);

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
        if let Some(ref ini) = dynamic_ini {
            generator = generator.with_dynamic_ini(ini.clone());
        }

        // Pre-fetch SQL metadata if SQL is remote
        if let Some(metadata) = self
            .fetch_sql_metadata_if_needed(profile, platform, dynamic_ini.as_ref(), "temp_profile")
            .await
        {
            generator = generator.with_sql_metadata(metadata);
        }

        // Apply profile-level settings (vault_key, pipelines, test_suite, env)
        generator.apply_profile_settings(profile);

        // Apply platform config
        generator.apply_platform(platform);

        // Apply all services and infra with profile data
        generator.apply_services(graph, profile, active_services);

        // Handle multi-project if present
        if let Some(ref multi_config) = profile.multi_project {
            // 1. Add nested glean_apps_canary section (only platform + SQL)
            let multi_profile_data = self
                .generate_multi_project_profile(profile, multi_config, dynamic_ini.as_ref())
                .await?;

            // 2. Inject multi-project services into top-level services map
            // so they appear as first-class Tilt resources (not just nested config)
            let top_level_services = multi_profile_data.services.clone();

            generator.set_multi_project(multi_profile_data);
            generator.inject_top_level_services(top_level_services);
        }

        generator.generate()
    }

    /// Generate nested multi-project profile data (platform info + SQL + services)
    async fn generate_multi_project_profile(
        &self,
        _primary_profile: &Profile,
        multi_config: &devdock_core::profile::MultiProjectConfig,
        dynamic_ini: Option<&ParsedIniConfig>,
    ) -> Result<crate::profile_generator::MultiProjectProfile> {
        use crate::profile_generator::{
            apply_bastion_port_forwards, build_remote_sql_config, InfraProfileEntry,
            MultiProjectProfile,
        };

        let multi_project_name = "glean-apps-canary";

        // Create platform for multi-project
        let multi_platform = crate::platform::create_platform_with_multi_project(
            "google",
            multi_project_name.to_string(),
            dynamic_ini,
            true,
        )
        .await?;

        // Build the nested infra map.
        //
        // Python's SqlServiceDependency.update_temp_profile rebinds the target dict
        // to temp_profile['glean_apps_canary'] when multi_project=True and then
        // populates infra.sql (cloud_sql_args / bastion+instances+ports+use_iam_auth)
        // and infra.bastion (local_to_remote_ports). The Rust path previously just
        // shallow-copied the source YAML config, so the nested SQL block came out
        // empty and no bastion port forwards were set up — meaning host.docker.internal
        // :3311/3312 had nothing listening for multi-project services like
        // debug_endpoint. This restores that behavior.
        let mut infra: HashMap<String, InfraProfileEntry> = HashMap::new();
        if let Some(sql_config) = multi_config.infra.get("sql") {
            let infra_type = match sql_config.deployment_type {
                devdock_core::types::DeploymentType::Local => "local",
                devdock_core::types::DeploymentType::Remote => "remote",
            }
            .to_string();

            let mut sql_extras = sql_config.config.clone();

            if sql_config.deployment_type == devdock_core::types::DeploymentType::Remote {
                // Strip local-only keys and apply the remote-SQL logic.
                sql_extras.retain(|k, _| {
                    !matches!(
                        k.as_str(),
                        "copy_tables"
                            | "mode"
                            | "full_cleanup"
                            | "copy_keyval"
                            | "copy_where_queries"
                    )
                });

                // The multi-project has its own SQL instance names (e.g.
                // `glean-apps-canary:us-central1:...`) stored in a different GCS
                // bucket's dynamic.ini. Using the primary project's dynamic.ini
                // here would emit the wrong connection strings. Fetch the
                // multi-project's own dynamic.ini and fall back to the primary
                // one only if that load fails.
                let multi_ini = self.load_dynamic_ini_for_project(multi_project_name).await;
                let effective_ini = multi_ini.as_ref().or(dynamic_ini);

                if let Some(ini) = effective_ini {
                    match crate::sql::SqlUtils::fetch_remote_metadata(
                        multi_project_name,
                        "google",
                        multi_platform.zone(),
                        ini,
                    )
                    .await
                    {
                        Ok(metadata) => {
                            let outputs = build_remote_sql_config(
                                &metadata,
                                Some(ini),
                                "google",
                                true, // is_multi_project
                            );
                            for (k, v) in outputs.sql_extras {
                                sql_extras.insert(k, v);
                            }
                            if !outputs.bastion_port_forwards.is_empty() {
                                let entry = infra
                                    .entry("bastion".to_string())
                                    .or_insert_with(|| InfraProfileEntry {
                                        infra_type: "local".to_string(),
                                        extra: HashMap::new(),
                                    });
                                apply_bastion_port_forwards(
                                    &mut entry.extra,
                                    &outputs.bastion_port_forwards,
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: Failed to fetch SQL metadata for multi-project nested profile ({}): {}",
                                multi_project_name, e
                            );
                        }
                    }
                } else {
                    eprintln!(
                        "Warning: dynamic.ini not available; skipping remote SQL expansion for multi-project nested profile"
                    );
                }
            }

            infra.insert(
                "sql".to_string(),
                InfraProfileEntry { infra_type, extra: sql_extras },
            );
        }

        // Build service entries for multi-project services
        let services = self.build_multi_project_service_entries(multi_config).await?;

        // Get platform details
        let temp_profile = crate::platform::TempProfile::default();
        let mut temp_profile_mut = temp_profile;
        multi_platform.update_temp_profile(&mut temp_profile_mut);

        Ok(MultiProjectProfile {
            project: temp_profile_mut.project,
            platform: temp_profile_mut.platform,
            region: temp_profile_mut.region,
            zone: temp_profile_mut.zone,
            project_number: temp_profile_mut.project_number,
            infra,
            services,
        })
    }

    /// Build ServiceProfileEntry map for multi-project services.
    /// Used both for the nested glean_apps_canary section and for top-level injection.
    async fn build_multi_project_service_entries(
        &self,
        multi_config: &devdock_core::profile::MultiProjectConfig,
    ) -> Result<HashMap<String, crate::profile_generator::ServiceProfileEntry>> {
        use crate::profile_generator::ServiceProfileEntry;

        let multi_project_name = "glean-apps-canary";

        let multi_profile = Profile {
            project: multi_project_name.to_string(),
            platform: devdock_core::types::Platform::Google,
            infra: multi_config.infra.clone(),
            services: multi_config.services.clone(),
            pipelines: HashMap::new(),
            test_suite: HashMap::new(),
            env: HashMap::new(),
            ini_overrides: Vec::new(),
            multi_project: None,
        };

        let multi_graph = DependencyGraph::build(&multi_profile, &self.workspace_root).await?;

        let mut services = HashMap::new();

        for node in multi_graph.nodes() {
            if node.component_type == crate::dependency_graph::ComponentType::Service
                && multi_config.services.contains_key(&node.name)
            {
                let profile_service = multi_profile.services.get(&node.name);

                let service_type =
                    if node.config.is_local() { "local".to_string() } else { "remote".to_string() };

                let mut env = profile_service
                    .and_then(|s| if s.env.is_empty() { None } else { Some(s.env.clone()) })
                    .unwrap_or_default();

                env.insert("USE_DEVDOCK_INI".to_string(), "true".to_string());
                env.insert("IS_DEVDOCK".to_string(), "true".to_string());
                let devdock_id = std::env::var("DEVDOCK_ID").unwrap_or_default();
                if devdock_id.is_empty() {
                    tracing::warn!("DEVDOCK_ID env var is empty during config generation — services will have no session correlation");
                }
                env.insert("DEVDOCK_ID".to_string(), devdock_id);

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

                let extra = profile_service.map(|s| s.config.clone()).unwrap_or_default();

                services.insert(
                    node.name.clone(),
                    ServiceProfileEntry {
                        service_type,
                        active: true,
                        port: None,
                        env: Some(env),
                        hot_reload,
                        script: None,
                        service: None,
                        depends_on_services: None,
                        timeout: None,
                        extra,
                    },
                );
            }
        }

        Ok(services)
    }

    /// Generate watch files for hot reload
    async fn generate_watch_files(&self, graph: &DependencyGraph) -> Result<()> {
        match self.watch_strategy {
            WatchStrategy::Skip => {
                println!("      └─ Watch file generation skipped");
                Ok(())
            }
            WatchStrategy::Static => self.generate_watch_files_static(graph).await,
            WatchStrategy::Cquery => self.generate_watch_files_cquery(graph).await,
            WatchStrategy::Hybrid => {
                // Generate static first (fast)
                self.generate_watch_files_static(graph).await?;

                // Spawn background task to refine with cquery (non-blocking)
                println!("      └─ Spawning background cquery refinement...");
                self.spawn_background_cquery_refinement(graph).await;

                Ok(())
            }
        }
    }

    /// Generate watch files using static glob patterns (instant, no bazel)
    async fn generate_watch_files_static(&self, graph: &DependencyGraph) -> Result<()> {
        use crate::dependency_graph::ComponentType;
        use crate::service_config::ServiceType as ConfigServiceType;

        let generator = WatchPatternGenerator::new(self.workspace_root.clone())?;

        // Filter: Only local services (not infra, not remote)
        let services: Vec<(String, WatchServiceType, Option<String>)> = graph
            .nodes()
            .filter(|n| {
                // Only include services (not infra/pipeline/test_suite)
                n.component_type == ComponentType::Service
                // Only include local deployment (not remote)
                && n.config.deployment_local
            })
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
            println!("      └─ No local services to generate watch files for");
            return Ok(());
        }

        println!(
            "      └─ Generating static watch patterns for {} local service(s)",
            services.len()
        );
        let start = std::time::Instant::now();

        generator.generate_all_watch_files(&services).await?;

        println!(
            "      └─ Static watch patterns generated in {:.2}ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
        Ok(())
    }

    /// Generate watch files using bazel cquery (accurate but slow)
    async fn generate_watch_files_cquery(&self, graph: &DependencyGraph) -> Result<()> {
        use crate::dependency_graph::ComponentType;

        // Filter: Only local services with bazel targets
        let services_with_targets: HashMap<String, String> = graph
            .nodes()
            .filter(|n| {
                // Only include services (not infra/pipeline/test_suite)
                n.component_type == ComponentType::Service
                // Only include local deployment (not remote)
                && n.config.deployment_local
                // Must have a bazel target
                && n.config.bazel_target.is_some()
            })
            .filter_map(|n| n.config.bazel_target.as_ref().map(|t| (n.name.clone(), t.clone())))
            .collect();

        if services_with_targets.is_empty() {
            println!("      └─ No local services with bazel targets for cquery");
            return Ok(());
        }

        let engine = BazelQueryEngine::new(self.workspace_root.clone());
        engine.generate_watch_files(services_with_targets).await
    }

    /// Spawn background task to refine watch files with bazel cquery
    ///
    /// This runs in the background and updates watch files when complete.
    /// The main process doesn't wait for this - DevDock starts immediately.
    ///
    /// Logs are written to ~/.devdock/background-cquery.log to avoid cluttering Tilt UI.
    async fn spawn_background_cquery_refinement(&self, graph: &DependencyGraph) {
        use crate::dependency_graph::ComponentType;
        use std::io::Write;

        // Filter: Only local services with bazel targets
        let services_with_targets: HashMap<String, String> = graph
            .nodes()
            .filter(|n| {
                // Only include services (not infra/pipeline/test_suite)
                n.component_type == ComponentType::Service
                // Only include local deployment (not remote)
                && n.config.deployment_local
                // Must have a bazel target
                && n.config.bazel_target.is_some()
            })
            .filter_map(|n| n.config.bazel_target.as_ref().map(|t| (n.name.clone(), t.clone())))
            .collect();

        if services_with_targets.is_empty() {
            println!("      └─ No local services with bazel targets for cquery refinement");
            return;
        }

        let workspace_root = self.workspace_root.clone();
        let service_count = services_with_targets.len();

        // Setup log file path
        let log_file = std::env::var("HOME")
            .ok()
            .map(|h| std::path::PathBuf::from(h).join(".devdock/background-cquery.log"));

        // Spawn detached background task
        tokio::spawn(async move {
            let start = std::time::Instant::now();

            // Helper to log to file
            let log = |msg: String| {
                if let Some(ref path) = log_file {
                    if let Ok(mut file) =
                        std::fs::OpenOptions::new().create(true).append(true).open(path)
                    {
                        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                        let _ = writeln!(file, "[{timestamp}] {msg}");
                    }
                }
            };

            log(format!("Starting cquery for {service_count} local service(s)"));

            // Use silent engine to avoid console spam
            let engine = BazelQueryEngine::new_silent(workspace_root);
            match engine.generate_watch_files(services_with_targets).await {
                Ok(()) => {
                    let msg = format!(
                        "✅ Refined watch files with cquery in {:.2}s",
                        start.elapsed().as_secs_f64()
                    );
                    log(msg);
                    // Success is silent - no console output to avoid cluttering Tilt UI
                }
                Err(e) => {
                    let msg = format!("❌ Cquery refinement failed: {e}");
                    log(msg.clone());
                    // Only print errors to console (after Tilt starts, but important to see)
                    eprintln!("\n⚠️  Background cquery failed (static patterns still active). Check ~/.devdock/background-cquery.log");
                }
            }
        });

        // Don't await - let it run in background
        println!("      └─ Background cquery starting (logs → ~/.devdock/background-cquery.log)");
    }
}
