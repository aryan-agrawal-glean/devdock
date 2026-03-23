//! Backend trait definitions.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Backend error type.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("Backend not initialized")]
    NotInitialized,
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Operation failed: {0}")]
    Operation(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type BackendResult<T> = std::result::Result<T, BackendError>;

/// Configuration passed to backends during initialization.
#[derive(Debug, Clone, Default)]
pub struct BackendConfig {
    pub settings: HashMap<String, serde_json::Value>,
    pub env: HashMap<String, String>,
    pub workspace_root: String,
    pub data_dir: String,
}

impl BackendConfig {
    pub fn get_string(&self, key: &str) -> Option<String> {
        self.settings.get(key)?.as_str().map(String::from)
    }

    pub fn get_string_or(&self, key: &str, default: &str) -> String {
        self.get_string(key).unwrap_or_else(|| default.to_string())
    }
}

// ─── Config Storage ────────────────────────────────────────────────────────────

/// Backend for fetching configuration files (local, GCS, S3).
#[async_trait]
pub trait ConfigStorageBackend: Send + Sync {
    fn name(&self) -> &str;
    async fn initialize(&mut self, config: BackendConfig) -> BackendResult<()>;
    async fn fetch(&self, key: &str) -> BackendResult<Vec<u8>>;
    async fn fetch_string(&self, key: &str) -> BackendResult<String> {
        let bytes = self.fetch(key).await?;
        String::from_utf8(bytes).map_err(|e| BackendError::Operation(format!("Invalid UTF-8: {e}")))
    }
    async fn exists(&self, key: &str) -> bool;
}

// ─── Telemetry Sync ────────────────────────────────────────────────────────────

/// Telemetry event (collected by core, synced by backend).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TelemetryEvent {
    SessionStart { session_id: String, profile: String, mode: String },
    SessionEnd { session_id: String, duration_secs: u64, success: bool },
    BootstrapComplete { session_id: String, bootstrap_time_ms: u64 },
    Error { session_id: String, message: String, context: HashMap<String, String> },
}

/// Backend for syncing telemetry (stdout, BigQuery, Datadog).
#[async_trait]
pub trait TelemetrySyncBackend: Send + Sync {
    fn name(&self) -> &str;
    async fn initialize(&mut self, config: BackendConfig) -> BackendResult<()>;
    async fn emit(&self, event: &TelemetryEvent) -> BackendResult<()>;
    async fn flush(&self) -> BackendResult<()>;
}

// ─── Diagnostics ───────────────────────────────────────────────────────────────

/// Backend for bug reporting (stdout, Jira, GitHub Issues).
#[async_trait]
pub trait DiagnosticsBackend: Send + Sync {
    fn name(&self) -> &str;
    async fn initialize(&mut self, config: BackendConfig) -> BackendResult<()>;
    async fn report(
        &self,
        title: &str,
        body: &str,
        context: &HashMap<String, String>,
    ) -> BackendResult<String>;
}

// ─── Remote Provider ───────────────────────────────────────────────────────────

/// Backend for remote development (SSH, Coder, Codespaces).
#[async_trait]
pub trait RemoteProviderBackend: Send + Sync {
    fn name(&self) -> &str;
    async fn initialize(&mut self, config: BackendConfig) -> BackendResult<()>;
    async fn exec(&self, workspace: &str, command: &str) -> BackendResult<String>;
    async fn copy_to(
        &self,
        workspace: &str,
        local: &std::path::Path,
        remote: &str,
    ) -> BackendResult<()>;
}

// ─── Built-in Backends ─────────────────────────────────────────────────────────

/// Stdout telemetry sync (default for OSS).
pub struct StdoutTelemetry;

#[async_trait]
impl TelemetrySyncBackend for StdoutTelemetry {
    fn name(&self) -> &str {
        "stdout"
    }
    async fn initialize(&mut self, _config: BackendConfig) -> BackendResult<()> {
        Ok(())
    }
    async fn emit(&self, event: &TelemetryEvent) -> BackendResult<()> {
        match event {
            TelemetryEvent::SessionStart { session_id, profile, mode } => {
                tracing::info!("Session started: id={session_id} profile={profile} mode={mode}")
            }
            TelemetryEvent::SessionEnd { session_id, duration_secs, success } => tracing::info!(
                "Session ended: id={session_id} duration={duration_secs}s success={success}"
            ),
            TelemetryEvent::BootstrapComplete { session_id, bootstrap_time_ms } => {
                tracing::info!("Bootstrap: id={session_id} time={bootstrap_time_ms}ms")
            }
            TelemetryEvent::Error { session_id, message, .. } => {
                tracing::info!("Error: id={session_id} msg={message}")
            }
        }
        Ok(())
    }
    async fn flush(&self) -> BackendResult<()> {
        Ok(())
    }
}

/// Local filesystem config storage (default for OSS).
pub struct LocalStorage {
    data_dir: Option<std::path::PathBuf>,
}

impl LocalStorage {
    pub fn new() -> Self {
        Self { data_dir: None }
    }
}

impl Default for LocalStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConfigStorageBackend for LocalStorage {
    fn name(&self) -> &str {
        "local"
    }
    async fn initialize(&mut self, config: BackendConfig) -> BackendResult<()> {
        let dir = if config.data_dir.is_empty() {
            dirs::home_dir()
                .ok_or_else(|| BackendError::Config("No home dir".into()))?
                .join(".devdock/config")
        } else {
            std::path::PathBuf::from(&config.data_dir).join("config")
        };
        std::fs::create_dir_all(&dir).ok();
        self.data_dir = Some(dir);
        Ok(())
    }
    async fn fetch(&self, key: &str) -> BackendResult<Vec<u8>> {
        let dir = self.data_dir.as_ref().ok_or(BackendError::NotInitialized)?;
        std::fs::read(dir.join(key)).map_err(|e| BackendError::NotFound(format!("{key}: {e}")))
    }
    async fn exists(&self, key: &str) -> bool {
        self.data_dir.as_ref().map(|d| d.join(key).exists()).unwrap_or(false)
    }
}

/// Stdout diagnostics (default for OSS).
pub struct StdoutDiagnostics;

#[async_trait]
impl DiagnosticsBackend for StdoutDiagnostics {
    fn name(&self) -> &str {
        "stdout"
    }
    async fn initialize(&mut self, _config: BackendConfig) -> BackendResult<()> {
        Ok(())
    }
    async fn report(
        &self,
        title: &str,
        body: &str,
        context: &HashMap<String, String>,
    ) -> BackendResult<String> {
        println!("Bug Report: {title}");
        println!("{body}");
        if !context.is_empty() {
            println!("Context: {context:?}");
        }
        Ok("printed to stdout".into())
    }
}
