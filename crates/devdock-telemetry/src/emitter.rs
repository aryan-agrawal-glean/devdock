//! Telemetry emitter trait for pluggable backends.

use crate::{Result, TelemetrySnapshot};
use async_trait::async_trait;

/// Trait for telemetry emission backends.
///
/// Implement this trait to support different telemetry destinations
/// (BigQuery, local files, Prometheus, etc.)
#[async_trait]
pub trait TelemetryEmitter: Send + Sync {
    /// Get the name of this emitter for logging.
    fn name(&self) -> &str;

    /// Check if the emitter is properly configured and ready.
    async fn is_ready(&self) -> bool;

    /// Emit a telemetry snapshot to the backend.
    async fn emit(&self, snapshot: &TelemetrySnapshot) -> Result<EmitResult>;

    /// Flush any buffered data (for batching implementations).
    async fn flush(&self) -> Result<()>;

    /// Close the emitter and clean up resources.
    async fn close(&self) -> Result<()>;
}

/// Result of an emit operation.
#[derive(Debug, Clone)]
pub struct EmitResult {
    /// Number of metrics successfully emitted.
    pub success_count: usize,
    /// Number of metrics that failed to emit.
    pub failure_count: usize,
    /// Error messages for failed emissions.
    pub errors: Vec<String>,
}

impl EmitResult {
    pub fn success(count: usize) -> Self {
        EmitResult { success_count: count, failure_count: 0, errors: vec![] }
    }

    pub fn partial(success: usize, failures: usize, errors: Vec<String>) -> Self {
        EmitResult { success_count: success, failure_count: failures, errors }
    }

    pub fn failure(error: String) -> Self {
        EmitResult { success_count: 0, failure_count: 1, errors: vec![error] }
    }

    pub fn is_success(&self) -> bool {
        self.failure_count == 0
    }
}

/// A no-op emitter for testing or when telemetry is disabled.
#[allow(dead_code)]
pub struct NoopEmitter;

#[async_trait]
impl TelemetryEmitter for NoopEmitter {
    fn name(&self) -> &'static str {
        "noop"
    }

    async fn is_ready(&self) -> bool {
        true
    }

    async fn emit(&self, snapshot: &TelemetrySnapshot) -> Result<EmitResult> {
        Ok(EmitResult::success(snapshot.total_metrics_count()))
    }

    async fn flush(&self) -> Result<()> {
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

/// An emitter that writes to local JSON files for debugging.
#[allow(dead_code)]
pub struct FileEmitter {
    output_dir: std::path::PathBuf,
}

#[allow(dead_code)]
impl FileEmitter {
    pub fn new(output_dir: impl Into<std::path::PathBuf>) -> Self {
        FileEmitter { output_dir: output_dir.into() }
    }
}

#[async_trait]
impl TelemetryEmitter for FileEmitter {
    fn name(&self) -> &'static str {
        "file"
    }

    async fn is_ready(&self) -> bool {
        std::fs::create_dir_all(&self.output_dir).is_ok()
    }

    async fn emit(&self, snapshot: &TelemetrySnapshot) -> Result<EmitResult> {
        use chrono::Utc;

        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let filename = self.output_dir.join(format!("telemetry_{timestamp}.json"));

        let json = serde_json::to_string_pretty(snapshot)?;
        std::fs::write(&filename, json)?;

        tracing::info!("Wrote telemetry to {:?}", filename);
        Ok(EmitResult::success(snapshot.total_metrics_count()))
    }

    async fn flush(&self) -> Result<()> {
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}
