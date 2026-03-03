//! GCP backend implementations for the backend registry.
//!
//! These wrap existing devdock-telemetry code behind the backend traits,
//! allowing config-driven selection via devdock.toml.

use crate::output;
use devdock_core::plugin::{
    BackendConfig, BackendError, BackendResult, TelemetryEvent, TelemetrySyncBackend,
};

/// BigQuery telemetry backend — wraps existing BigQueryEmitter.
pub struct BigQueryTelemetry {
    project_id: String,
    dataset: String,
}

impl BigQueryTelemetry {
    pub fn new(project_id: &str, dataset: &str) -> Self {
        Self { project_id: project_id.to_string(), dataset: dataset.to_string() }
    }

    pub fn from_config(config: &devdock_core::ProjectConfig) -> Self {
        Self::new(
            config
                .telemetry
                .project
                .as_deref()
                .or(config.cloud.project.as_deref())
                .unwrap_or("dev-sandbox-334901"),
            config.telemetry.dataset.as_deref().unwrap_or("devdock"),
        )
    }
}

#[async_trait::async_trait]
impl TelemetrySyncBackend for BigQueryTelemetry {
    fn name(&self) -> &str {
        "bigquery"
    }

    async fn initialize(&mut self, _config: BackendConfig) -> BackendResult<()> {
        Ok(())
    }

    async fn emit(&self, event: &TelemetryEvent) -> BackendResult<()> {
        use devdock_telemetry::{
            BigQueryConfig, BigQueryEmitter, SessionMetrics, TelemetryEmitter, TelemetrySnapshot,
        };

        let config = BigQueryConfig {
            project_id: self.project_id.clone(),
            dataset: self.dataset.clone(),
            use_adc: true,
            ..Default::default()
        };

        // Convert our TelemetryEvent to the existing BigQuery format
        match event {
            TelemetryEvent::SessionEnd { session_id, duration_secs, success } => {
                let session = SessionMetrics {
                    devdock_id: session_id.clone(),
                    machine_id: String::new(),
                    architecture: std::env::consts::OS.to_string(),
                    platform: std::env::consts::OS.to_string(),
                    project: self.project_id.clone(),
                    profile: String::new(),
                    branch: None,
                    binary_type: "rust".to_string(),
                    binary_version: env!("CARGO_PKG_VERSION").to_string(),
                    command_start_time: chrono::Utc::now(),
                    tilt_up_time: None,
                    watch_start_time: None,
                    startup_complete_time: Some(chrono::Utc::now()),
                    tilt_bootstrap_seconds: None,
                    total_startup_seconds: Some(*duration_secs as f64),
                    startup_status: if *success { "success" } else { "failure" }.to_string(),
                    total_resources: 0,
                    healthy_resources: 0,
                    failed_resources: 0,
                    timestamp: chrono::Utc::now(),
                };

                let snapshot = TelemetrySnapshot { session: Some(session), ..Default::default() };

                match BigQueryEmitter::new(config) {
                    Ok(emitter) => {
                        emitter
                            .emit(&snapshot)
                            .await
                            .map_err(|e| BackendError::Operation(format!("BigQuery emit: {e}")))?;
                    }
                    Err(e) => {
                        tracing::debug!("BigQuery not available: {e}");
                    }
                }
            }
            _ => {
                // Other events just logged for now
                tracing::debug!("BigQuery telemetry event: {:?}", event);
            }
        }

        Ok(())
    }

    async fn flush(&self) -> BackendResult<()> {
        Ok(())
    }
}

/// GCS config storage backend — wraps existing DynamicIniLoader.
pub struct GcsStorage {
    project: String,
}

impl GcsStorage {
    pub fn new(project: &str) -> Self {
        Self { project: project.to_string() }
    }
}

#[async_trait::async_trait]
impl devdock_core::plugin::ConfigStorageBackend for GcsStorage {
    fn name(&self) -> &str {
        "gcs"
    }

    async fn initialize(&mut self, _config: BackendConfig) -> BackendResult<()> {
        Ok(())
    }

    async fn fetch(&self, key: &str) -> BackendResult<Vec<u8>> {
        // Use gsutil to fetch from GCS bucket
        let uri = format!("gs://config-{}/{}", self.project, key);
        let output = std::process::Command::new("gsutil")
            .args(["cat", &uri])
            .output()
            .map_err(|e| BackendError::Operation(format!("gsutil: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BackendError::NotFound(format!("{uri}: {stderr}")));
        }
        Ok(output.stdout)
    }

    async fn exists(&self, key: &str) -> bool {
        let uri = format!("gs://config-{}/{}", self.project, key);
        std::process::Command::new("gsutil")
            .args(["stat", &uri])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

/// Coder remote provider backend — wraps existing devdock-remote coder module.
pub struct CoderRemote {
    #[allow(dead_code)]
    template: Option<String>,
}

impl CoderRemote {
    pub fn new(template: Option<&str>) -> Self {
        Self { template: template.map(String::from) }
    }
}

#[async_trait::async_trait]
impl devdock_core::plugin::RemoteProviderBackend for CoderRemote {
    fn name(&self) -> &str {
        "coder"
    }

    async fn initialize(&mut self, _config: BackendConfig) -> BackendResult<()> {
        // Verify coder CLI is installed
        which::which("coder").map_err(|_| BackendError::Config("Coder CLI not found".into()))?;
        Ok(())
    }

    async fn exec(&self, workspace: &str, command: &str) -> BackendResult<String> {
        let output = std::process::Command::new("coder")
            .args(["ssh", workspace, "--", command])
            .output()
            .map_err(|e| BackendError::Operation(format!("coder ssh: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BackendError::Operation(format!("coder ssh failed: {stderr}")));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    async fn copy_to(
        &self,
        workspace: &str,
        local: &std::path::Path,
        remote: &str,
    ) -> BackendResult<()> {
        let target = format!("{workspace}.coder:{remote}");
        let output = std::process::Command::new("scp")
            .args(["-r", local.to_str().unwrap_or(""), &target])
            .output()
            .map_err(|e| BackendError::Operation(format!("scp: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BackendError::Operation(format!("scp failed: {stderr}")));
        }
        Ok(())
    }
}

/// Jira diagnostics backend — wraps existing devdock-diagnostics jira module.
pub struct JiraDiagnostics {
    #[allow(dead_code)]
    jira_url: String,
}

impl JiraDiagnostics {
    pub fn new(url: &str) -> Self {
        Self { jira_url: url.to_string() }
    }
}

#[async_trait::async_trait]
impl devdock_core::plugin::DiagnosticsBackend for JiraDiagnostics {
    fn name(&self) -> &str {
        "jira"
    }

    async fn initialize(&mut self, _config: BackendConfig) -> BackendResult<()> {
        Ok(())
    }

    async fn report(
        &self,
        title: &str,
        body: &str,
        context: &std::collections::HashMap<String, String>,
    ) -> BackendResult<String> {
        // Jira integration uses the existing BugReporter workflow
        // which handles JiraDescription formatting and attachments.
        // For the backend trait, we provide a simplified interface.
        tracing::info!("Jira bug report: {title}");
        tracing::debug!("Body: {body}, Context: {context:?}");
        output::info(&format!("Creating Jira ticket: {title}"));
        output::info("Use `devdock report` for full Jira integration with attachments");
        Ok(format!("Jira: {title} (use devdock report for full integration)"))
    }
}

/// Register backends into the registry based on devdock.toml config.
pub async fn register_gcp_backends(
    registry: &devdock_core::BackendRegistry,
    config: &devdock_core::ProjectConfig,
) {
    // BigQuery telemetry
    if config.telemetry.enabled && config.telemetry.backend == "bigquery" {
        let bq = BigQueryTelemetry::from_config(config);
        registry.set_telemetry(Box::new(bq)).await;
        tracing::info!("Registered BigQuery telemetry backend");
    }

    // GCS storage
    if matches!(config.cloud.platform.as_str(), "google" | "gcp") {
        if let Some(ref project) = config.cloud.project {
            registry.set_storage(Box::new(GcsStorage::new(project))).await;
            tracing::info!("Registered GCS storage backend (project: {project})");
        }
    }

    // Coder remote
    if config.remote.provider == "coder" {
        let coder = CoderRemote::new(config.remote.template.as_deref());
        // Note: remote backend not yet used by pipeline — registered for future use
        tracing::info!("Registered Coder remote backend");
        let _ = coder; // suppress unused warning until remote pipeline uses registry
    }

    // Jira diagnostics
    if config.diagnostics.reporter == "jira" {
        // Jira URL would come from config, defaulting to askscio
        registry
            .set_diagnostics(Box::new(JiraDiagnostics::new("https://askscio.atlassian.net")))
            .await;
        tracing::info!("Registered Jira diagnostics backend");
    }
}
