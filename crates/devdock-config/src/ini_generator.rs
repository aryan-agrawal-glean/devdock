//! DevDock INI configuration generator.

use crate::dependency_graph::ServiceNode;
use crate::ini_parser::ParsedIniConfig;
use crate::platform::{IniConfig, Platform};
use crate::service_config::ServiceConfig;
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
    /// Platform name (aws, google, azure)
    pub platform: String,
    /// AWS account ID (for AWS-specific config)
    pub account_id: Option<String>,
    /// SQL remote metadata (for remote SQL configuration)
    pub sql_metadata: Option<crate::sql::SqlRemoteMetadata>,
    /// OpenSearch namespace from profile (defaults to elasticsearch-2-namespace if not set)
    pub opensearch_namespace: Option<String>,
}

impl IniGeneratorContext {
    pub fn from_profile(profile: &Profile) -> Self {
        let has_quality_pipelines = profile.pipelines.contains_key("quality_pipelines");
        // Don't set multi_project flag just because profile has multi_project section
        // This flag should only be true when generating scio_apps.ini itself
        let multi_project = false;

        // Extract opensearch namespace from profile if present
        let opensearch_namespace = profile.infra.get("opensearch").and_then(|opensearch| {
            opensearch.config.get("namespace").and_then(|ns| ns.as_str().map(String::from))
        });

        Self {
            multi_project,
            has_quality_pipelines,
            project: profile.project.clone(),
            platform: profile.platform.to_string(),
            account_id: None,
            sql_metadata: None,
            opensearch_namespace,
        }
    }

    /// Set SQL metadata for remote SQL configuration
    pub fn with_sql_metadata(mut self, metadata: crate::sql::SqlRemoteMetadata) -> Self {
        self.sql_metadata = Some(metadata);
        self
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
        Self { config, context: IniGeneratorContext::default() }
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
    pub fn with_base_config_and_context(
        _base: &ParsedIniConfig,
        context: IniGeneratorContext,
    ) -> Self {
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

        // Caching Service
        config.set("cachingservice", "serviceHost", "caching_service.devdock.local:50073");

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

        // Tools
        config.set("tools", "enableHostedComposio", "false");
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
            self.config.set(&override_entry.section, &override_entry.key, &override_entry.val);
        }
    }

    /// Apply profile-level INI overrides
    ///
    /// These are applied last and take precedence over all other configurations.
    pub fn apply_profile_overrides(&mut self, overrides: &[devdock_core::IniOverride]) {
        for override_entry in overrides {
            self.config.set(&override_entry.section, &override_entry.key, &override_entry.val);
        }
    }

    /// Apply dependency-specific configuration
    pub fn apply_dependency(&mut self, node: &ServiceNode) {
        match node.name.as_str() {
            // Infrastructure
            "sql" => self.apply_sql_config(node),
            "redis" => self.apply_redis_config(node),
            "valkey" => self.apply_valkey_config(node),
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
            "llm_proxy_service" => self.apply_llm_proxy_service_config(node),
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
                format!("localhost:{sql_port}")
            } else {
                format!("host.docker.internal:{sql_port}")
            };

            // Basic config for local SQL
            self.config.set("sql", "useSecretsManagerForPassword", "false");
            self.config.set("sql", "useDbIAMAuth", "false");
            self.config.set("sql", "useCloudSqlConnectorInQe", "false");

            // Instance list depends on project type (matches Python sql_service_dependency.py special cases)
            // - glean-apps-canary/scio-apps: use internal + frontendInstance
            // - Standard: use frontendInstance + backendInstance
            let instances: Vec<&str> = if self.context.project == "glean-apps-canary"
                || self.context.project == "scio-apps"
            {
                vec!["internal", "frontendInstance"]
            } else {
                vec!["frontendInstance", "backendInstance"]
            };

            for instance in instances {
                let instance_prefix = format!("{instance}.");

                self.config.set("sql", &format!("{instance_prefix}db.password"), "");
                self.config.set("sql", &format!("{instance_prefix}db.user"), "root");
                self.config.set("sql", &format!("{instance_prefix}db.host"), &sql_host);
            }

            // Set googlecloud.instanceName for each instance type
            if self.context.project == "glean-apps-canary" || self.context.project == "scio-apps" {
                self.config.set("sql", "internal.googlecloud.instanceName", "");
                self.config.set("sql", "frontendInstance.googlecloud.instanceName", "");
                self.config.set("sql", "googlecloud.instanceName", "");
                // Set unprefixed fields for special projects
                self.config.set("sql", "db.user", "root");
                self.config.set("sql", "db.password", "");
                self.config.set("sql", "db.host", &sql_host);
            } else {
                self.config.set("sql", "frontendInstance.googlecloud.instanceName", "");
                self.config.set("sql", "backendInstance.googlecloud.instanceName", "");
            }

            // AWS-specific fields for local SQL (matches Python behavior)
            if self.context.platform == "aws" {
                for instance in &["frontendInstance", "backendInstance"] {
                    let instance_prefix =
                        if instance.is_empty() { String::new() } else { format!("{instance}.") };

                    self.config.set("sql", &format!("aws.{instance_prefix}rootSecretArn"), "");
                    self.config.set(
                        "sql",
                        &format!("{instance_prefix}aws.instanceEndpoint"),
                        &sql_host,
                    );
                }
            }
        } else {
            // Remote SQL configuration
            // Port base for remote SQL proxy
            let curr_port = if self.context.multi_project { 3311 } else { 3307 };

            // Check if we have metadata about public IP
            let use_public_ip =
                self.context.sql_metadata.as_ref().map(|m| m.use_public_ip).unwrap_or(false);

            // Set auth configuration based on public IP
            if use_public_ip
                && !matches!(
                    self.context.project.as_str(),
                    "glean-apps-canary" | "scio-engineering"
                )
            {
                // Public IP in test projects: use glean_local user, no IAM
                self.config.set("sql", "useDbIAMAuth", "false");
                self.config.set("sql", "useCloudSqlConnectorInQe", "false");
            }
            // For bastion path (private IP): don't override useDbIAMAuth or
            // useSecretsManagerForPassword. The bastion's Cloud SQL Auth Proxy
            // handles IAM auth at the transport level, so the JDBC driver
            // should use standard auth. Leave these from default.ini so
            // profile ini_overrides can control them.

            // Canary's bastion runs `cloud-sql-proxy --auto-iam-authn` as
            // `bastion@glean-apps-canary.iam.gserviceaccount.com`, so the
            // proxy authenticates to Cloud SQL as the DB user `bastion`.
            // The canary ini otherwise ships `db.user=root / db.password=
            // scio`, which Cloud SQL will reject for the proxy-injected
            // session. Override user to the proxy's IAM identity; the
            // password is ignored by the proxy but blanked for clarity.
            let is_bastion_available =
                self.context.sql_metadata.as_ref().is_some_and(|m| m.is_bastion_available);
            let is_canary_iam_bastion = self.context.multi_project
                && self.context.project == "glean-apps-canary"
                && !use_public_ip
                && is_bastion_available;

            // Set host for remote SQL (goes through cloud-sql-proxy or bastion).
            // For canary/scio-apps the canary bastion launches the proxy as
            // `sql-1?port=3311` (frontendInstance) then `sql-internal?port=
            // 3312` (internal); the order here must match so
            // `SqlFrontendKeyValStore` (frontendInstance) actually hits
            // sql-1 where `keyval` lives.
            let instances: &[&str] = if self.context.project == "glean-apps-canary"
                || self.context.project == "scio-apps"
            {
                &["frontendInstance", "internal"]
            } else {
                &["frontendInstance", "backendInstance"]
            };

            for (idx, instance) in instances.iter().enumerate() {
                let port = curr_port + idx as u16;
                self.config.set(
                    "sql",
                    &format!("{}.db.host", instance),
                    format!("host.docker.internal:{}", port),
                );
                self.config.set("sql", &format!("{}.googlecloud.instanceName", instance), "");

                if use_public_ip
                    && !matches!(
                        self.context.project.as_str(),
                        "glean-apps-canary" | "scio-engineering"
                    )
                {
                    self.config.set("sql", &format!("{}.db.user", instance), "glean_local");
                }

                if is_canary_iam_bastion {
                    self.config.set("sql", &format!("{}.db.user", instance), "bastion");
                    self.config.set("sql", &format!("{}.db.password", instance), "");
                }
            }

            if self.context.project == "glean-apps-canary" || self.context.project == "scio-apps" {
                // Unprefixed `sql.db.*` routes to the same instance as
                // `frontendInstance` (canary/scio-apps ini maps both
                // `sql.googlecloud.instanceName` and
                // `sql.frontendInstance.googlecloud.instanceName` to sql-1).
                // DBs without an explicit `*.db.instance` (e.g. `keyval`)
                // fall back here, so they MUST hit the primary port, not
                // `sql-internal`.
                self.config.set("sql", "db.host", format!("host.docker.internal:{}", curr_port));
                self.config.set("sql", "googlecloud.instanceName", "");
                if is_canary_iam_bastion {
                    self.config.set("sql", "db.user", "bastion");
                    self.config.set("sql", "db.password", "");
                }
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

    fn apply_valkey_config(&mut self, _node: &ServiceNode) {
        self.config.set("redis", "useValkeyBackend", "true");
        self.config.set("valkey", "host", "host.docker.internal");
        self.config.set("valkey", "port", "6382");
        self.config.set("valkey", "useTls", "false");
        for instance in &["fe_cache", "fe_persistent", "be_cache", "be_persistent"] {
            self.config.set("valkey", &format!("{instance}.discoveryEndpoints"), "");
        }
    }

    fn apply_opensearch_config(&mut self, node: &ServiceNode) {
        // OpenSearch config is generated for BOTH local and remote (matches Python behavior)
        let port = node.config.env.get("PORT").unwrap_or(&"9200".to_string()).clone();

        self.config.set("indexing", "index.name", "index-1");
        self.config.set("indexing", "server.url", format!("http://host.docker.internal:{port}"));
        self.config.set("entitybuilder", "index.name", "entity-index-1");
        self.config.set("elastic_plugin", "hotswapJarPluginTag", "");
        self.config.set("elastic_plugin", "hotswapJarPluginConfig", "");

        // Namespace-based config - use namespace from context or default to elasticsearch-2-namespace
        let namespace =
            self.context.opensearch_namespace.as_deref().unwrap_or("elasticsearch-2-namespace");

        self.config.set(
            namespace,
            "indexing.server.url",
            format!("http://host.docker.internal:{port}"),
        );
        self.config.set(namespace, "indexing.index.name", "index-1");
        self.config.set(namespace, "entitybuilder.index.name", "entity-index-1");

        // Cluster mapping with namespace from context
        let cluster_mapping = format!(
            r#"{{"primary": [{{"namespace": "{}", "cluster_type": "StandardCluster"}}], "staging": [], "old": []}}"#,
            namespace
        );
        self.config.set("opensearch", "clusterMapping", &cluster_mapping);
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
            format!("https://local.glean.com:{port}")
        } else {
            // For remote, would need to get from dynamic config
            // For now, use local
            format!("https://local.glean.com:{port}")
        };

        self.config.set("queryapi", "baseURL", &glean_base_url);
        self.config.set("queryapi", "baseInternalURL", &glean_base_url);
        self.config.set("queryapi", "baseExternalURL", "https://local.glean.com:8321");
        // Note: preferClusterBaseUrl is set by deployment_wizard, not QE

        // AWS-specific timeouts for local QE (matches Python behavior)
        if node.config.is_local() && self.context.platform == "aws" {
            self.config.set("queryapi", "search.temporaryTimeoutMillisOverride", "300000");
            self.config.set("queryapi", "autocomplete.timeoutMillis", "300000");
        }
    }

    fn apply_task_push_config(&mut self, node: &ServiceNode) {
        let port = node.config.env.get("PORT").unwrap_or(&"7992".to_string()).clone();

        self.config.set("task_push", "baseURL", format!("http://host.docker.internal:{port}"));
        self.config.set(
            "task_push",
            "internalLBBaseURL",
            format!("http://host.docker.internal:{port}"),
        );
        self.config.set("task_push", "rabbitmq.connections", "5");
    }

    fn apply_crawler_config(&mut self, node: &ServiceNode) {
        let port = node.config.env.get("PORT").unwrap_or(&"7961".to_string()).clone();

        self.config.set(
            "task_push",
            "crawler.baseURL",
            format!("http://host.docker.internal:{port}"),
        );
        self.config.set("local", "taskHandler.port", "7961");
        self.config.set("chat", "fileUpload.clamAVScan.enabled", "false");
        self.config.set("net", "defaultInternetGatewayHttpProxy", "");
        self.config.set("crawl", "useCachedSecretStore", "false");
    }

    fn apply_scholastic_config(&mut self, node: &ServiceNode) {
        let port = node.config.env.get("PORT").unwrap_or(&"8090".to_string()).clone();

        self.config.set(
            "queryapi",
            "scholastic.url",
            format!("http://host.docker.internal:{port}"),
        );
    }

    fn apply_py_agents_config(&mut self, node: &ServiceNode) {
        let port = node.config.env.get("PORT").unwrap_or(&"8091".to_string()).clone();

        self.config.set("queryapi", "pyAgents.url", format!("http://host.docker.internal:{port}"));
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
            self.config.set(
                "queryapi",
                "datasourceEvents.url",
                format!("http://local.glean.com:{port}"),
            );
        } else {
            let port = node.config.env.get("PORT").unwrap_or(&"7962".to_string()).clone();
            self.config.set(
                "queryapi",
                "datasourceEvents.url",
                format!("http://host.docker.internal:{port}"),
            );
        }
    }

    fn apply_notebook_sandbox_config(&mut self, node: &ServiceNode) {
        if node.config.is_local() {
            let port = node.config.env.get("PORT").unwrap_or(&"5050".to_string()).clone();
            self.config.set(
                "code_interpreter",
                "notebook_sandbox_host_url",
                format!("http://local.glean.com:{port}"),
            );
            self.config.set("chat", "fileUpload.clamAVScan.enabled", "false");
        }
    }

    fn apply_tempo_config(&mut self, node: &ServiceNode) {
        if !node.config.is_local() {
            self.config.set("queryapi", "otel.otlpEndpoint", "http://host.docker.internal:4318");
            self.config.set(
                "queryapi",
                "otel.tempoQueryFrontendEndpoint",
                "http://host.docker.internal:3100",
            );
        }
    }

    fn apply_llm_proxy_service_config(&mut self, node: &ServiceNode) {
        if node.config.is_local() {
            let port = node.config.env.get("PORT").unwrap_or(&"7980".to_string()).clone();
            self.config.set(
                "deploy",
                "llm_proxy_service.url",
                format!("http://local.glean.com:{port}"),
            );
        } else {
            let port = node.config.env.get("PORT").unwrap_or(&"7980".to_string()).clone();
            self.config.set(
                "deploy",
                "llm_proxy_service.url",
                format!("http://host.docker.internal:{port}"),
            );
        }
    }

    fn apply_git_crawler_config(&mut self, node: &ServiceNode) {
        if node.config.is_local() {
            let port = node.config.env.get("PORT").unwrap_or(&"7976".to_string()).clone();
            self.config.set(
                "deploy",
                "k8s.git_crawler.url",
                format!("http://host.docker.internal:{port}/git-crawler"),
            );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dependency_graph::ComponentType;

    /// Create a test SQL service node with local deployment
    fn create_local_sql_node() -> ServiceNode {
        let mut config = ServiceConfig::default();
        config.set_deployment_local(true);
        ServiceNode { name: "sql".to_string(), config, component_type: ComponentType::Infra }
    }

    /// Create a test SQL service node with remote deployment
    fn create_remote_sql_node() -> ServiceNode {
        let mut config = ServiceConfig::default();
        config.set_deployment_local(false);
        ServiceNode { name: "sql".to_string(), config, component_type: ComponentType::Infra }
    }

    fn create_local_infra_node(name: &str) -> ServiceNode {
        let mut config = ServiceConfig::default();
        config.set_deployment_local(true);
        ServiceNode { name: name.to_string(), config, component_type: ComponentType::Infra }
    }

    /// Create test context for a given project
    fn create_test_context(project: &str) -> IniGeneratorContext {
        IniGeneratorContext {
            project: project.to_string(),
            platform: "google".to_string(),
            account_id: None,
            ..Default::default()
        }
    }

    #[test]
    fn test_local_sql_config_standard_project() {
        // Standard projects should use frontendInstance and backendInstance
        let context = create_test_context("some-standard-project");
        let mut generator = IniGenerator::with_context(context);
        let node = create_local_sql_node();
        generator.apply_dependency(&node);

        let ini_output = generator.generate();

        // Should have frontendInstance and backendInstance
        assert!(
            ini_output.contains("frontendInstance.db.host"),
            "Standard project should have frontendInstance"
        );
        assert!(
            ini_output.contains("backendInstance.db.host"),
            "Standard project should have backendInstance"
        );
        assert!(
            ini_output.contains("frontendInstance.googlecloud.instanceName"),
            "Standard project should have frontendInstance.googlecloud.instanceName"
        );
        assert!(
            ini_output.contains("backendInstance.googlecloud.instanceName"),
            "Standard project should have backendInstance.googlecloud.instanceName"
        );

        // Should NOT have internal instance
        assert!(
            !ini_output.contains("internal.db.host"),
            "Standard project should NOT have internal instance"
        );
    }

    #[test]
    fn test_local_sql_config_glean_apps_canary() {
        // glean-apps-canary should use internal and frontendInstance (NOT backendInstance)
        let context = create_test_context("glean-apps-canary");
        let mut generator = IniGenerator::with_context(context);
        let node = create_local_sql_node();
        generator.apply_dependency(&node);

        let ini_output = generator.generate();

        // Should have internal and frontendInstance
        assert!(
            ini_output.contains("internal.db.host"),
            "glean-apps-canary should have internal instance"
        );
        assert!(
            ini_output.contains("frontendInstance.db.host"),
            "glean-apps-canary should have frontendInstance"
        );
        assert!(
            ini_output.contains("internal.googlecloud.instanceName"),
            "glean-apps-canary should have internal.googlecloud.instanceName"
        );
        assert!(
            ini_output.contains("frontendInstance.googlecloud.instanceName"),
            "glean-apps-canary should have frontendInstance.googlecloud.instanceName"
        );
        // Should also have unprefixed googlecloud.instanceName
        assert!(
            ini_output.contains("\ngooglecloud.instanceName")
                || ini_output.starts_with("googlecloud.instanceName"),
            "glean-apps-canary should have unprefixed googlecloud.instanceName"
        );

        // Should NOT have backendInstance
        assert!(
            !ini_output.contains("backendInstance.db.host"),
            "glean-apps-canary should NOT have backendInstance"
        );
    }

    #[test]
    fn test_local_sql_config_scio_apps() {
        // scio-apps should use internal and frontendInstance (same as glean-apps-canary)
        let context = create_test_context("scio-apps");
        let mut generator = IniGenerator::with_context(context);
        let node = create_local_sql_node();
        generator.apply_dependency(&node);

        let ini_output = generator.generate();

        // Should have internal and frontendInstance
        assert!(ini_output.contains("internal.db.host"), "scio-apps should have internal instance");
        assert!(
            ini_output.contains("frontendInstance.db.host"),
            "scio-apps should have frontendInstance"
        );

        // Should NOT have backendInstance
        assert!(
            !ini_output.contains("backendInstance.db.host"),
            "scio-apps should NOT have backendInstance"
        );
    }

    #[test]
    fn test_remote_sql_config_glean_apps_canary() {
        // Remote SQL for glean-apps-canary should also use internal + frontendInstance
        let context = create_test_context("glean-apps-canary");
        let mut generator = IniGenerator::with_context(context);
        let node = create_remote_sql_node();
        generator.apply_dependency(&node);

        let ini_output = generator.generate();

        // Should have internal and frontendInstance
        assert!(
            ini_output.contains("internal.db.host"),
            "Remote glean-apps-canary should have internal instance"
        );
        assert!(
            ini_output.contains("frontendInstance.db.host"),
            "Remote glean-apps-canary should have frontendInstance"
        );

        // Should NOT have backendInstance
        assert!(
            !ini_output.contains("backendInstance.db.host"),
            "Remote glean-apps-canary should NOT have backendInstance"
        );
    }

    #[test]
    fn test_remote_sql_config_standard_project() {
        // Remote SQL for standard projects should use frontendInstance + backendInstance
        let context = create_test_context("some-standard-project");
        let mut generator = IniGenerator::with_context(context);
        let node = create_remote_sql_node();
        generator.apply_dependency(&node);

        let ini_output = generator.generate();

        // Should have frontendInstance and backendInstance
        assert!(
            ini_output.contains("frontendInstance.db.host"),
            "Remote standard project should have frontendInstance"
        );
        assert!(
            ini_output.contains("backendInstance.db.host"),
            "Remote standard project should have backendInstance"
        );

        // Should NOT have internal instance
        assert!(
            !ini_output.contains("internal.db.host"),
            "Remote standard project should NOT have internal instance"
        );
    }

    #[test]
    fn test_tools_section_in_defaults() {
        // Verify that [tools] section is included in generated INI
        let generator = IniGenerator::new();
        let ini_output = generator.generate();

        assert!(ini_output.contains("[tools]"), "Generated INI should contain [tools] section");
        assert!(
            ini_output.contains("enableHostedComposio = false"),
            "Generated INI should have enableHostedComposio = false"
        );
    }

    #[test]
    fn test_local_valkey_clears_cloud_discovery_endpoints() {
        let mut generator = IniGenerator::new();
        for instance in &["fe_cache", "fe_persistent", "be_cache", "be_persistent"] {
            generator.config.set(
                "valkey",
                &format!("{instance}.discoveryEndpoints"),
                "10.0.0.1:6379",
            );
        }

        generator.apply_dependency(&create_local_infra_node("valkey"));

        let ini_output = generator.generate();
        for instance in &["fe_cache", "fe_persistent", "be_cache", "be_persistent"] {
            assert!(ini_output.contains(&format!("{instance}.discoveryEndpoints = ")));
            assert!(!ini_output.contains(&format!("{instance}.discoveryEndpoints = 10.0.0.1:6379")));
        }
    }
}
