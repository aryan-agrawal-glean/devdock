//! GCP backend implementations for the backend registry.
//!
//! These wrap existing devdock-telemetry code behind the backend traits,
//! allowing config-driven selection via devdock.toml.

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

/// Register GCP backends into the registry based on config.
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
}
