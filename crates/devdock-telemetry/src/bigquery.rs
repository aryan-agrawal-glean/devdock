//! BigQuery emitter for telemetry data.
//!
//! Uses the BigQuery REST API with service account authentication
//! to insert telemetry data into BigQuery tables.

use crate::{
    emitter::{EmitResult, TelemetryEmitter},
    models::*,
    Result, TelemetryError,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// BigQuery table configuration.
#[derive(Debug, Clone)]
pub struct BigQueryConfig {
    /// GCP project ID (e.g., "dev-sandbox-334901")
    pub project_id: String,
    /// BigQuery dataset (e.g., "devdock")
    pub dataset: String,
    /// Path to service account key file (optional, uses ADC if not set)
    pub service_account_key: Option<String>,
    /// Whether to use Application Default Credentials
    pub use_adc: bool,
    /// Batch size for inserts (default: 500)
    pub batch_size: usize,
}

impl Default for BigQueryConfig {
    fn default() -> Self {
        BigQueryConfig {
            project_id: "dev-sandbox-334901".to_string(),
            dataset: "devdock".to_string(),
            service_account_key: None,
            use_adc: true,
            batch_size: 500,
        }
    }
}

/// Table names in BigQuery.
pub struct TableNames {
    pub session_stats: String,
    pub resource_stats: String,
    pub startup_stats: String,
    pub host_stats: String,
    pub docker_stats: String,
    pub failure_logs: String,
}

impl TableNames {
    pub fn new(project: &str, dataset: &str) -> Self {
        TableNames {
            session_stats: format!("{}.{}.devdock_session_stats", project, dataset),
            resource_stats: format!("{}.{}.resource_stats", project, dataset),
            startup_stats: format!("{}.{}.startup_stats", project, dataset),
            host_stats: format!("{}.{}.host_stats", project, dataset),
            docker_stats: format!("{}.{}.docker_stats", project, dataset),
            failure_logs: format!("{}.{}.resource_failure_logs", project, dataset),
        }
    }
}

/// BigQuery streaming insert request.
#[derive(Debug, Serialize)]
struct InsertRequest<T> {
    rows: Vec<InsertRow<T>>,
    #[serde(rename = "skipInvalidRows")]
    skip_invalid_rows: bool,
    #[serde(rename = "ignoreUnknownValues")]
    ignore_unknown_values: bool,
}

#[derive(Debug, Serialize)]
struct InsertRow<T> {
    json: T,
}

/// BigQuery streaming insert response.
#[derive(Debug, Deserialize)]
struct InsertResponse {
    #[serde(rename = "insertErrors")]
    insert_errors: Option<Vec<InsertError>>,
}

#[derive(Debug, Deserialize)]
struct InsertError {
    index: u32,
    errors: Vec<ErrorDetail>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ErrorDetail {
    reason: String,
    message: String,
}

/// BigQuery emitter using REST API.
pub struct BigQueryEmitter {
    config: BigQueryConfig,
    client: Client,
    tables: TableNames,
    access_token: Arc<RwLock<Option<String>>>,
}

impl BigQueryEmitter {
    pub fn new(config: BigQueryConfig) -> Result<Self> {
        let tables = TableNames::new(&config.project_id, &config.dataset);
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| TelemetryError::BigQuery(e.to_string()))?;

        Ok(BigQueryEmitter { config, client, tables, access_token: Arc::new(RwLock::new(None)) })
    }

    /// Get or refresh the access token.
    async fn get_access_token(&self) -> Result<String> {
        {
            let token = self.access_token.read().await;
            if let Some(t) = token.as_ref() {
                return Ok(t.clone());
            }
        }

        let token = self.fetch_access_token().await?;
        {
            let mut token_lock = self.access_token.write().await;
            *token_lock = Some(token.clone());
        }
        Ok(token)
    }

    async fn fetch_access_token(&self) -> Result<String> {
        if self.config.use_adc {
            self.fetch_adc_token().await
        } else if let Some(key_path) = &self.config.service_account_key {
            self.fetch_sa_token(key_path).await
        } else {
            Err(TelemetryError::Config("No authentication method configured".to_string()))
        }
    }

    async fn fetch_adc_token(&self) -> Result<String> {
        let output = tokio::process::Command::new("gcloud")
            .args(["auth", "print-access-token"])
            .output()
            .await
            .map_err(|e| TelemetryError::BigQuery(format!("Failed to get ADC token: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TelemetryError::BigQuery(format!("gcloud auth failed: {}", stderr)));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    async fn fetch_sa_token(&self, _key_path: &str) -> Result<String> {
        Err(TelemetryError::Config("Service account key auth not yet implemented".to_string()))
    }

    async fn insert_rows<T: Serialize + Send + Sync>(
        &self,
        table_id: &str,
        rows: &[T],
    ) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }

        let token = self.get_access_token().await?;
        let parts: Vec<&str> = table_id.split('.').collect();
        if parts.len() != 3 {
            return Err(TelemetryError::Config(format!("Invalid table ID format: {}", table_id)));
        }

        let url = format!(
            "https://bigquery.googleapis.com/bigquery/v2/projects/{}/datasets/{}/tables/{}/insertAll",
            parts[0], parts[1], parts[2]
        );

        let insert_rows: Vec<InsertRow<&T>> = rows.iter().map(|r| InsertRow { json: r }).collect();

        let request = InsertRequest {
            rows: insert_rows,
            skip_invalid_rows: false,
            ignore_unknown_values: true,
        };

        debug!("Inserting {} rows into {}", rows.len(), table_id);

        let response = self.client.post(&url).bearer_auth(&token).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(TelemetryError::BigQuery(format!(
                "BigQuery insert failed ({}): {}",
                status, body
            )));
        }

        let insert_response: InsertResponse = response.json().await?;

        if let Some(errors) = insert_response.insert_errors {
            let error_count = errors.len();
            for err in errors.iter().take(3) {
                warn!("BigQuery insert error at row {}: {:?}", err.index, err.errors);
            }
            return Ok(rows.len() - error_count);
        }

        info!("Successfully inserted {} rows into {}", rows.len(), table_id);
        Ok(rows.len())
    }
}

#[async_trait]
impl TelemetryEmitter for BigQueryEmitter {
    fn name(&self) -> &str {
        "bigquery"
    }

    async fn is_ready(&self) -> bool {
        match self.get_access_token().await {
            Ok(_) => true,
            Err(e) => {
                warn!("BigQuery not ready: {}", e);
                false
            }
        }
    }

    async fn emit(&self, snapshot: &TelemetrySnapshot) -> Result<EmitResult> {
        let mut success_count = 0;
        let mut failure_count = 0;
        let mut errors = Vec::new();

        if let Some(session) = &snapshot.session {
            match self.insert_rows(&self.tables.session_stats, &[session]).await {
                Ok(n) => success_count += n,
                Err(e) => {
                    failure_count += 1;
                    errors.push(format!("session_stats: {}", e));
                }
            }
        }

        if !snapshot.resource_statuses.is_empty() {
            match self.insert_rows(&self.tables.resource_stats, &snapshot.resource_statuses).await {
                Ok(n) => success_count += n,
                Err(e) => {
                    failure_count += snapshot.resource_statuses.len();
                    errors.push(format!("resource_stats: {}", e));
                }
            }
        }

        if !snapshot.resource_startups.is_empty() {
            match self.insert_rows(&self.tables.startup_stats, &snapshot.resource_startups).await {
                Ok(n) => success_count += n,
                Err(e) => {
                    failure_count += snapshot.resource_startups.len();
                    errors.push(format!("startup_stats: {}", e));
                }
            }
        }

        if let Some(host) = &snapshot.host {
            match self.insert_rows(&self.tables.host_stats, &[host]).await {
                Ok(n) => success_count += n,
                Err(e) => {
                    failure_count += 1;
                    errors.push(format!("host_stats: {}", e));
                }
            }
        }

        if !snapshot.docker.is_empty() {
            match self.insert_rows(&self.tables.docker_stats, &snapshot.docker).await {
                Ok(n) => success_count += n,
                Err(e) => {
                    failure_count += snapshot.docker.len();
                    errors.push(format!("docker_stats: {}", e));
                }
            }
        }

        if !snapshot.failures.is_empty() {
            match self.insert_rows(&self.tables.failure_logs, &snapshot.failures).await {
                Ok(n) => success_count += n,
                Err(e) => {
                    failure_count += snapshot.failures.len();
                    errors.push(format!("failure_logs: {}", e));
                }
            }
        }

        if errors.is_empty() {
            Ok(EmitResult::success(success_count))
        } else {
            Ok(EmitResult::partial(success_count, failure_count, errors))
        }
    }

    async fn flush(&self) -> Result<()> {
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}
