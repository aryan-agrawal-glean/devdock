//! External plugin protocol — process-based plugins via JSON stdin/stdout.
//!
//! Allows custom backends written in any language (Python, Go, Node, etc.).
//! The plugin is an executable that speaks JSON-lines over stdin/stdout.
//!
//! Protocol:
//! ```text
//! → {"method":"initialize","config":{"key":"value"}}
//! ← {"status":"ok"}
//! → {"method":"emit","event":{"type":"SessionStart","session_id":"abc"}}
//! ← {"status":"ok"}
//! → {"method":"shutdown"}
//! ← {"status":"ok"}
//! ```

use super::traits::*;
use std::io::{BufRead, Write};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

/// External telemetry plugin — communicates via JSON over stdin/stdout.
pub struct ExternalTelemetryPlugin {
    plugin_path: String,
    child: Mutex<Option<Child>>,
}

impl ExternalTelemetryPlugin {
    pub fn new(plugin_path: &str) -> Self {
        Self { plugin_path: plugin_path.to_string(), child: Mutex::new(None) }
    }

    fn send_message(&self, msg: &serde_json::Value) -> BackendResult<serde_json::Value> {
        let mut guard =
            self.child.lock().map_err(|_| BackendError::Operation("mutex poisoned".into()))?;
        let child = guard.as_mut().ok_or(BackendError::NotInitialized)?;

        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| BackendError::Operation("stdin not available".into()))?;

        let msg_str = serde_json::to_string(msg)
            .map_err(|e| BackendError::Operation(format!("JSON serialize: {e}")))?;

        writeln!(stdin, "{msg_str}")
            .map_err(|e| BackendError::Operation(format!("write to plugin: {e}")))?;
        stdin.flush().map_err(|e| BackendError::Operation(format!("flush: {e}")))?;

        // Read response
        let stdout = child
            .stdout
            .as_mut()
            .ok_or_else(|| BackendError::Operation("stdout not available".into()))?;

        let mut response = String::new();
        let mut reader = std::io::BufReader::new(stdout);
        reader
            .read_line(&mut response)
            .map_err(|e| BackendError::Operation(format!("read from plugin: {e}")))?;

        serde_json::from_str(&response)
            .map_err(|e| BackendError::Operation(format!("JSON parse response: {e}")))
    }
}

#[async_trait::async_trait]
impl TelemetrySyncBackend for ExternalTelemetryPlugin {
    fn name(&self) -> &str {
        "external"
    }

    async fn initialize(&mut self, config: BackendConfig) -> BackendResult<()> {
        let child = Command::new(&self.plugin_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                BackendError::Config(format!("Failed to start plugin '{}': {e}", self.plugin_path))
            })?;

        *self.child.lock().map_err(|_| BackendError::Operation("mutex poisoned".into()))? =
            Some(child);

        // Send initialize message
        let init_msg = serde_json::json!({
            "method": "initialize",
            "config": config.settings,
        });

        let response = self.send_message(&init_msg)?;
        if response.get("status").and_then(|s| s.as_str()) != Some("ok") {
            return Err(BackendError::Config(format!(
                "Plugin initialization failed: {}",
                response
            )));
        }

        Ok(())
    }

    async fn emit(&self, event: &TelemetryEvent) -> BackendResult<()> {
        let msg = serde_json::json!({
            "method": "emit",
            "event": event,
        });

        let response = self.send_message(&msg)?;
        if response.get("status").and_then(|s| s.as_str()) != Some("ok") {
            return Err(BackendError::Operation(format!("Plugin emit failed: {}", response)));
        }

        Ok(())
    }

    async fn flush(&self) -> BackendResult<()> {
        Ok(())
    }
}

impl Drop for ExternalTelemetryPlugin {
    fn drop(&mut self) {
        // Send shutdown message (best effort)
        let msg = serde_json::json!({"method": "shutdown"});
        let _ = self.send_message(&msg);

        // Wait for process to exit
        if let Ok(mut guard) = self.child.lock() {
            if let Some(ref mut child) = *guard {
                let _ = child.wait();
            }
        }
    }
}
