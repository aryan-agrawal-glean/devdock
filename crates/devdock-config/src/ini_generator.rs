//! DevDock INI configuration generator.

use crate::ini_parser::ParsedIniConfig;
use crate::platform::{IniConfig, Platform};
use crate::service_config::ServiceConfig;
use crate::dependency_graph::ServiceNode;
use devdock_core::profile::Profile;

/// Context for INI generation that affects how configs are generated
#[derive(Debug, Default)]
pub struct IniGeneratorContext {
    /// Whether this is a multi-project setup (e.g., glean-apps-canary)
    pub multi_project: bool,
    /// Whether quality_pipelines are enabled (uses localhost instead of host.docker.internal)
    pub has_quality_pipelines: bool,
    /// Project name (for special project handling)
    pub project: String,
}

impl IniGeneratorContext {
    pub fn from_profile(profile: &Profile) -> Self {
        let has_quality_pipelines = profile.pipelines.contains_key("quality_pipelines");
        let multi_project = profile.multi_project.is_some();

        Self {
            multi_project,
            has_quality_pipelines,
            project: profile.project.clone(),
        }
    }
}

/// Generator for devdock.ini configuration
pub struct IniGenerator {
    config: IniConfig,
    context: IniGeneratorContext,
}

impl IniGenerator {
    pub fn new() -> Self {
        let mut config = IniConfig::new();
        Self::add_defaults(&mut config);
        Self {
            config,
            context: IniGeneratorContext::default(),
        }
    }

    /// Create with context from profile
    pub fn with_context(context: IniGeneratorContext) -> Self {
        let mut generator = Self::new();
        generator.context = context;
        generator
    }

    /// Create with reference to dynamic.ini for reading values (NOT merging)
    /// 
    /// IMPORTANT: devdock.ini should ONLY contain local development overrides.
    /// Services read dynamic.ini first (base config from GCS), then devdock.ini overrides.
    /// We do NOT merge dynamic.ini into devdock.ini - that would duplicate everything.
    /// 
    /// The dynamic_ini parameter is used to READ values when we need to reference
    /// the production config (e.g., to get namespace names), but NOT to copy them.
    #[allow(dead_code)]
    pub fn with_dynamic_ini_ref(_dynamic_ini: &ParsedIniConfig) -> Self {
        // Don't merge - just create with defaults
        Self::new()
    }

    /// Create with context (the correct way to create the generator)
    /// 
    /// devdock.ini waterfall resolution:
    /// 1. Services read dynamic.ini (fetched from GCS, stored separately)
    /// 2. Services then read devdock.ini (local overrides only)
    /// 
    /// This method creates a generator that produces ONLY the overrides.
    pub fn with_base_config_and_context(_base: &ParsedIniConfig, context: IniGeneratorContext) -> Self {
        // IMPORTANT: Do NOT merge base config - devdock.ini should only have overrides
        let mut generator = Self::new();
        generator.context = context;
        generator
    }

    /// Set context
    pub fn set_context(&mut self, context: IniGeneratorContext) {
        self.context = context;
    }

    /// Add default configuration values
    fn add_defaults(config: &mut IniConfig) {
        // Storage
        config.set("storage", "caching.disabled", "false");

        // Memcached
        config.set("memcached", "discoveryEndpoints", "memcached.devdock.local:11211");

        // Google Drive
        config.set("gdrive", "contentIncrementalCrawlFrequencySecs", "1");
        config.set("gdrive", "crawl.activityFrequencySecs", "1");
        config.set("gdrive", "crawl.identityFrequencySecs", "1");

        // Slack
        config.set("slack", "contentIncrementalCrawlFrequencySecs", "1");
        config.set("slack", "identityCrawlFrequencySecs", "1");
        config.set("slack", "shouldDropBotMessagesForCrawl", "false");

        // MS Teams
        config.set("msteams", "contentIncrementalCrawlFrequencySecs", "1");
        config.set("msteams", "crawl.identityFrequencySecs", "1");

        // Box
        config.set("box", "contentIncrementalCrawlFrequencySecs", "1");
        config.set("box", "crawl.identityFrequencySecs", "1");
        config.set("box", "crawl.activityFrequencySecs", "1");

        // Hound
        config.set("hound", "baseURL", "");

        // O365
        config.set("o365", "crawl.identityFrequencySecs", "1");
        config.set("o365", "crawl.activityFrequencySecs", "1");
        config.set("o365onedrive", "crawl.contentIncrementalCrawlFrequencySecs", "1");

        // Config
        config.set("config", "refreshSecs", "864000");

        // Public proxy
        config.set("public_proxy", "enabled", "false");

        // DLP
        config.set("dlp", "continuous.findingsPublishIntervalMinutes", "5");
        config.set("dlp_multiple", "continuous.findingsPublishIntervalMinutes", "5");

        // Docbuilder
        config.set("docbuilder", "sharedLinkExpirationBufferSecs", "0");
        config.set("docbuilder", "streamingScan.full.scanFrequencyMins", "1440");
    }

    /// Apply platform-specific configuration
    pub fn apply_platform(&mut self, platform: &dyn Platform) {
        platform.update_devdock_config(&mut self.config);
    }

    /// Apply service configuration
    /// 
    /// NOTE: Environment variables are NOT written to devdock.ini.
    /// They belong in docker-compose.yaml or container runtime.
    /// devdock.ini only contains app configuration (INI overrides).
    pub fn apply_service(&mut self, _name: &str, service_config: &ServiceConfig) {
        // Only apply explicit INI overrides defined for the service
        // Do NOT write environment variables to devdock.ini
        for override_entry in &service_config.ini_overrides {
            self.config.set(
                &override_entry.section,
                &override_entry.key,
                &override_entry.val,
            );
        }
    }

    /// Apply dependency-specific configuration
    pub fn apply_dependency(&mut self, node: &ServiceNode) {
        match node.name.as_str() {
            // Infrastructure
            "sql" => self.apply_sql_config(node),
            "redis" => self.apply_redis_config(node),
            "opensearch" => self.apply_opensearch_config(node),
            "memcached" => self.apply_memcached_config(node),
            "rabbitmq" => self.apply_rabbitmq_config(node),
            "pubsub" => self.apply_pubsub_config(node),
            "servicebus" => self.apply_servicebus_config(node),

            // Services (alphabetically ordered)
            "crawler" => self.apply_crawler_config(node),
            "deployment_wizard" => self.apply_deployment_wizard_config(node),
            "docbuilder" => self.apply_docbuilder_config(node),
            "dse" => self.apply_dse_config(node),
            "git_crawler" => self.apply_git_crawler_config(node),
            "notebook_sandbox" => self.apply_notebook_sandbox_config(node),
            "py_agents" => self.apply_py_agents_config(node),
            "qe" => self.apply_qe_config(node),
            "scholastic" => self.apply_scholastic_config(node),
            "task_push" | "taskpush" => self.apply_task_push_config(node),
            "tempo" => self.apply_tempo_config(node),

            _ => {}
        }
    }

    fn apply_sql_config(&mut self, node: &ServiceNode) {
        if node.config.is_local() {
            // Use different port for multi-project setup
            let sql_port = if self.context.multi_project { "3319" } else { "3310" };

            // Determine host based on quality_pipelines (they run outside Docker)
            let sql_host = if self.context.has_quality_pipelines {
                format!("localhost:{}", sql_port)
            } else {
                format!("host.docker.internal:{}", sql_port)
            };

            // Basic config for local SQL
            self.config.set("sql", "useSecretsManagerForPassword", "false");
            self.config.set("sql", "useDbIAMAuth", "false");
            self.config.set("sql", "useCloudSqlConnectorInQe", "false");

            // Standard instances - only set local connection overrides
            // NOTE: googlecloud.instanceName should NOT be set here - it comes from dynamic.ini
            let instances = vec!["", "frontendInstance", "backendInstance"];

            for instance in instances {
                let instance_prefix = if instance.is_empty() {
                    String::new()
                } else {
                    format!("{}.", instance)
                };

                self.config.set("sql", &format!("{}db.password", instance_prefix), "");
                self.config.set("sql", &format!("{}db.user", instance_prefix), "root");
                // DO NOT set googlecloud.instanceName - it comes from dynamic.ini
                self.config.set("sql", &format!("{}db.host", instance_prefix), &sql_host);
            }

            // Special handling for glean-apps-canary and scio-apps
            if self.context.project == "glean-apps-canary" || self.context.project == "scio-apps" {
                self.config.set("sql", "db.user", "root");
                self.config.set("sql", "db.password", "");
                // DO NOT set googlecloud.instanceName - it comes from dynamic.ini
                self.config.set("sql", "db.host", &sql_host);
            }
        } else {
            // Remote SQL configuration
            // Port base for remote SQL proxy
            let curr_port = if self.context.multi_project { 3311 } else { 3307 };

            // For remote, we still set these but with different values
            // The actual cloud-sql-proxy setup happens in Tiltfile based on temp_profile.yaml
            self.config.set("sql", "useSecretsManagerForPassword", "true");
            self.config.set("sql", "useDbIAMAuth", "true");

            // Set host for remote SQL (goes through cloud-sql-proxy)
            // NOTE: googlecloud.instanceName should NOT be set here - it comes from dynamic.ini
            let instances = vec![("frontendInstance", curr_port), ("backendInstance", curr_port + 1)];

            for (instance, port) in instances {
                self.config.set("sql", &format!("{}.db.host", instance),
                    format!("host.docker.internal:{}", port));
                // DO NOT set googlecloud.instanceName - it comes from dynamic.ini
            }

            // Special handling for glean-apps-canary and scio-apps
            if self.context.project == "glean-apps-canary" || self.context.project == "scio-apps" {
                self.config.set("sql", "db.host", format!("host.docker.internal:{}", curr_port + 1));
                // DO NOT set googlecloud.instanceName - it comes from dynamic.ini
            }
        }
    }

    fn apply_redis_config(&mut self, node: &ServiceNode) {
        // Redis uses host.docker.internal for both cases since it runs in a container
        self.config.set("redis", "host", "host.docker.internal");
        self.config.set("redis", "internalLBHost", "host.docker.internal");

        if node.config.is_local() {
            self.config.set("redis", "port", "6380");
        } else {
            self.config.set("redis", "port", "6381");
        }
    }

    fn apply_opensearch_config(&mut self, node: &ServiceNode) {
        if node.config.is_local() {
            let port = node.config.env.get("PORT").unwrap_or(&"9200".to_string()).clone();

            self.config.set("indexing", "index.name", "index-1");
            self.config.set("indexing", "server.url", format!("http://host.docker.internal:{}", port));
            self.config.set("entitybuilder", "index.name", "entity-index-1");
            self.config.set("elastic_plugin", "hotswapJarPluginTag", "");
            self.config.set("elastic_plugin", "hotswapJarPluginConfig", "");

            // Namespace-based config (standard namespace convention)
            let namespace = "elasticsearch-1-namespace";
            self.config.set(namespace, "indexing.server.url", format!("http://host.docker.internal:{}", port));
            self.config.set(namespace, "indexing.index.name", "index-1");
            self.config.set(namespace, "entitybuilder.index.name", "entity-index-1");

            // Cluster mapping with standard namespace
            let cluster_mapping = r#"{"primary": [{"namespace": "elasticsearch-1-namespace", "cluster_type": "StandardCluster"}], "staging": [], "old": []}"#;
            self.config.set("opensearch", "clusterMapping", cluster_mapping);
        }
    }

    fn apply_memcached_config(&mut self, _node: &ServiceNode) {
        // Already set in defaults
    }

    fn apply_rabbitmq_config(&mut self, node: &ServiceNode) {
        if node.config.is_local() {
            self.config.set("rabbitmq", "host", "host.docker.internal");
            self.config.set("rabbitmq", "internalLBHost", "host.docker.internal");
            self.config.set("rabbitmq", "port", "5671");
            self.config.set("rabbitmq", "mgmtPort", "15671");
            self.config.set("rabbitmq", "useTls", "false");
        }
    }

    fn apply_pubsub_config(&mut self, node: &ServiceNode) {
        if node.config.is_local() {
            self.config.set("pubsub", "emulator_host", "pubsub.devdock.local:8085");
        }
    }

    fn apply_servicebus_config(&mut self, node: &ServiceNode) {
        if node.config.is_local() {
            self.config.set("servicebus", "host", "host.docker.internal");
            self.config.set("servicebus", "port", "5672");
        }
    }

    /// Generate the final INI string
    pub fn generate(self) -> String {
        self.config.as_string()
    }

    // ===== Service-Specific Config Methods =====

    fn apply_qe_config(&mut self, node: &ServiceNode) {
        let port = node.config.env.get("PORT").unwrap_or(&"7960".to_string()).clone();

        let glean_base_url = if node.config.is_local() {
            format!("https://local.glean.com:{}", port)
        } else {
            // For remote, would need to get from dynamic config
            // For now, use local
            format!("https://local.glean.com:{}", port)
        };

        self.config.set("queryapi", "baseURL", &glean_base_url);
        self.config.set("queryapi", "baseInternalURL", &glean_base_url);
        self.config.set("queryapi", "preferClusterBaseUrl", "false");
    }

    fn apply_task_push_config(&mut self, node: &ServiceNode) {
        let port = node.config.env.get("PORT").unwrap_or(&"7992".to_string()).clone();

        self.config.set("task_push", "baseURL", format!("http://host.docker.internal:{}", port));
        self.config.set("task_push", "internalLBBaseURL", format!("http://host.docker.internal:{}", port));
        self.config.set("task_push", "rabbitmq.connections", "5");
    }

    fn apply_crawler_config(&mut self, node: &ServiceNode) {
        let port = node.config.env.get("PORT").unwrap_or(&"7961".to_string()).clone();

        self.config.set("task_push", "crawler.baseURL", format!("http://host.docker.internal:{}", port));
        self.config.set("local", "taskHandler.port", "7961");
        self.config.set("chat", "fileUpload.clamAVScan.enabled", "false");
    }

    fn apply_scholastic_config(&mut self, node: &ServiceNode) {
        let port = node.config.env.get("PORT").unwrap_or(&"8090".to_string()).clone();

        self.config.set("queryapi", "scholastic.url", format!("http://host.docker.internal:{}", port));
    }

    fn apply_py_agents_config(&mut self, node: &ServiceNode) {
        let port = node.config.env.get("PORT").unwrap_or(&"8091".to_string()).clone();

        self.config.set("queryapi", "pyAgents.url", format!("http://host.docker.internal:{}", port));
    }

    fn apply_deployment_wizard_config(&mut self, _node: &ServiceNode) {
        self.config.set("queryapi", "preferClusterBaseUrl", "false");
        self.config.set("slack_global", "logAllEvents", "false");
    }

    fn apply_docbuilder_config(&mut self, _node: &ServiceNode) {
        // Docbuilder doesn't add INI config, only temp_profile modifications
        // which are handled by profile_generator
    }

    fn apply_dse_config(&mut self, node: &ServiceNode) {
        if node.config.is_local() {
            let port = node.config.env.get("PORT").unwrap_or(&"7962".to_string()).clone();
            self.config.set("queryapi", "datasourceEvents.url",
                format!("http://local.glean.com:{}", port));
        } else {
            let port = node.config.env.get("PORT").unwrap_or(&"7962".to_string()).clone();
            self.config.set("queryapi", "datasourceEvents.url",
                format!("http://host.docker.internal:{}", port));
        }
    }

    fn apply_notebook_sandbox_config(&mut self, node: &ServiceNode) {
        if node.config.is_local() {
            let port = node.config.env.get("PORT").unwrap_or(&"5050".to_string()).clone();
            self.config.set("code_interpreter", "notebook_sandbox_host_url",
                format!("http://local.glean.com:{}", port));
            self.config.set("chat", "fileUpload.clamAVScan.enabled", "false");
        }
    }

    fn apply_tempo_config(&mut self, node: &ServiceNode) {
        if !node.config.is_local() {
            self.config.set("queryapi", "otel.otlpEndpoint", "http://host.docker.internal:4318");
            self.config.set("queryapi", "otel.tempoQueryFrontendEndpoint", "http://host.docker.internal:3100");
        }
    }

    fn apply_git_crawler_config(&mut self, node: &ServiceNode) {
        if node.config.is_local() {
            let port = node.config.env.get("PORT").unwrap_or(&"7976".to_string()).clone();
            self.config.set("deploy", "computeengine.git_crawler.url",
                format!("http://host.docker.internal:{}/git-crawler", port));
            self.config.set("deploy", "git_crawler.machine_type", "e2-small");
            self.config.set("git", "vmCrawler.workspaceDirectory", "/mnt/git_crawler/workspace");
        }
    }
}

impl Default for IniGenerator {
    fn default() -> Self {
        Self::new()
    }
}

