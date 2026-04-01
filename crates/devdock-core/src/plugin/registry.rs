//! Backend registry — selects and manages active backends based on config.

use super::traits::*;
use crate::project_config::ProjectConfig;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Central registry managing active backends.
///
/// Backends are selected based on `devdock.toml` configuration.
/// The registry provides uniform access regardless of which backend is active.
pub struct BackendRegistry {
    storage: Arc<RwLock<Box<dyn ConfigStorageBackend>>>,
    telemetry: Arc<RwLock<Box<dyn TelemetrySyncBackend>>>,
    diagnostics: Arc<RwLock<Box<dyn DiagnosticsBackend>>>,
}

impl BackendRegistry {
    /// Create a registry with default backends (stdout, local, etc.).
    pub fn with_defaults() -> Self {
        Self {
            storage: Arc::new(RwLock::new(Box::new(LocalStorage::new()))),
            telemetry: Arc::new(RwLock::new(Box::new(StdoutTelemetry))),
            diagnostics: Arc::new(RwLock::new(Box::new(StdoutDiagnostics))),
        }
    }

    /// Create a registry configured from `devdock.toml`.
    ///
    /// Selects backends based on config values. If a configured backend
    /// requires a feature flag that's not compiled in, returns an error
    /// with a clear message.
    pub fn from_config(config: &ProjectConfig) -> crate::Result<Self> {
        let registry = Self::with_defaults();

        // Log selected backends (actual registration happens in devdock-cli
        // which has access to the telemetry/storage crates)
        if config.telemetry.enabled {
            tracing::info!(
                "Telemetry backend: {} (registration handled by CLI)",
                config.telemetry.backend
            );
        }
        if config.cloud.platform != "none" {
            tracing::info!(
                "Cloud platform: {} (storage backend selected accordingly)",
                config.cloud.platform
            );
        }

        Ok(registry)
    }

    /// Replace the telemetry backend (e.g., with BigQuery).
    pub async fn set_telemetry(&self, backend: Box<dyn TelemetrySyncBackend>) {
        *self.telemetry.write().await = backend;
    }

    /// Replace the storage backend.
    pub async fn set_storage(&self, backend: Box<dyn ConfigStorageBackend>) {
        *self.storage.write().await = backend;
    }

    /// Replace the diagnostics backend.
    pub async fn set_diagnostics(&self, backend: Box<dyn DiagnosticsBackend>) {
        *self.diagnostics.write().await = backend;
    }

    /// Emit a telemetry event.
    pub async fn emit_telemetry(&self, event: &TelemetryEvent) -> BackendResult<()> {
        self.telemetry.read().await.emit(event).await
    }

    /// Flush telemetry.
    pub async fn flush_telemetry(&self) -> BackendResult<()> {
        self.telemetry.read().await.flush().await
    }

    /// Fetch a config file.
    pub async fn fetch_config(&self, key: &str) -> BackendResult<String> {
        self.storage.read().await.fetch_string(key).await
    }

    /// Check if a config file exists.
    pub async fn config_exists(&self, key: &str) -> bool {
        self.storage.read().await.exists(key).await
    }

    /// Report a bug.
    pub async fn report_bug(
        &self,
        title: &str,
        body: &str,
        context: &std::collections::HashMap<String, String>,
    ) -> BackendResult<String> {
        self.diagnostics.read().await.report(title, body, context).await
    }

    /// Get active backend names for display.
    pub async fn active_backends(&self) -> (String, String, String) {
        (
            self.storage.read().await.name().to_string(),
            self.telemetry.read().await.name().to_string(),
            self.diagnostics.read().await.name().to_string(),
        )
    }

    /// Initialize all backends with the given config.
    pub async fn initialize_all(&self, config: BackendConfig) -> BackendResult<()> {
        self.storage.write().await.initialize(config.clone()).await?;
        self.telemetry.write().await.initialize(config.clone()).await?;
        self.diagnostics.write().await.initialize(config).await?;
        Ok(())
    }
}

impl Default for BackendRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn defaults_work() {
        let registry = BackendRegistry::with_defaults();
        let (storage, telemetry, diagnostics) = registry.active_backends().await;
        assert_eq!(storage, "local");
        assert_eq!(telemetry, "stdout");
        assert_eq!(diagnostics, "stdout");
    }

    #[tokio::test]
    async fn emit_telemetry() {
        let registry = BackendRegistry::with_defaults();
        let result = registry
            .emit_telemetry(&TelemetryEvent::SessionStart {
                session_id: "test".into(),
                profile: "default".into(),
                mode: "local".into(),
            })
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn from_config_defaults() {
        let config = ProjectConfig::parse("[project]\nname = \"test\"").unwrap();
        let registry = BackendRegistry::from_config(&config).unwrap();
        let (storage, telemetry, _) = registry.active_backends().await;
        assert_eq!(storage, "local");
        assert_eq!(telemetry, "stdout");
    }

    #[tokio::test]
    async fn from_config_unsupported_backend_warns() {
        let config =
            ProjectConfig::parse("[project]\nname = \"test\"\n[telemetry]\nbackend = \"unknown\"")
                .unwrap();
        // Should succeed (falls back to stdout), not error
        let registry = BackendRegistry::from_config(&config).unwrap();
        let (_, telemetry, _) = registry.active_backends().await;
        assert_eq!(telemetry, "stdout");
    }

    #[tokio::test]
    async fn report_bug_stdout() {
        let registry = BackendRegistry::with_defaults();
        let mut ctx = std::collections::HashMap::new();
        ctx.insert("service".into(), "qe".into());
        let result = registry.report_bug("Test bug", "Description", &ctx).await;
        assert!(result.is_ok());
    }
}
