//! DevDock Plugin SDK
//!
//! Build external plugins for DevDock in Rust. Plugins communicate with
//! the devdock binary via JSON-lines over stdin/stdout.
//!
//! # Example
//!
//! ```rust,ignore
//! use devdock_plugin_sdk::{Plugin, run};
//! use serde_json::Value;
//!
//! #[derive(Default)]
//! struct MyPlugin;
//!
//! impl Plugin for MyPlugin {
//!     fn name(&self) -> &str { "my-plugin" }
//!     fn initialize(&mut self, config: Value) { /* setup */ }
//!     fn handle(&self, method: &str, params: &Value) -> Value {
//!         serde_json::json!({"status": "ok"})
//!     }
//! }
//!
//! fn main() { run::<MyPlugin>(); }
//! ```

use serde_json::Value;
use std::io::{self, BufRead, Write};

/// Trait that all DevDock plugins implement.
///
/// Plugins receive lifecycle hooks as method calls. The full lifecycle:
///
/// - `"after_profile_loaded"` — profile YAML loaded
/// - `"before_infra_ready"` — before Docker/ports (auth, credentials)
/// - `"after_infra_ready"` — after Docker/ports ready
/// - `"before_config_generated"` — before config files (fetch remote configs)
/// - `"after_config_generated"` — after config files (write devdock.ini)
/// - `"before_tilt"` — before Tilt spawns (last-minute config)
/// - `"after_tilt"` — after Tilt is up (telemetry: session started)
/// - `"before_resource_start"` — before a resource starts (params: name, kind)
/// - `"after_resource_start"` — after a resource is healthy (params: name, kind)
/// - `"after_ready"` — all resources healthy (notifications)
/// - `"before_stop"` — Ctrl+C / exit signal (final telemetry flush)
/// - `"after_stop"` — after cleanup complete
/// - `"on_error"` — unrecoverable error
/// - `"on_report"` — `devdock report` command
///
/// Plugins only need to handle hooks they care about — unhandled hooks
/// should return `ok()`.
pub trait Plugin: Default {
    /// Plugin name (for logging).
    fn name(&self) -> &str;

    /// Called once with config from devdock.toml.
    fn initialize(&mut self, config: Value);

    /// Handle a method call. Called for lifecycle hooks and custom methods.
    ///
    /// Lifecycle hook params:
    /// - `"after_profile_loaded"` — `profile_name`, `workspace_root`, `platform`
    /// - `"before_infra_ready"` — `profile_name`, `instance_id`, `port_base`
    /// - `"before_config_generated"` — `profile_name`, `runtime_dir`, `workspace_root`
    /// - `"after_config_generated"` — `profile_name`, `runtime_dir`, `workspace_root`
    /// - `"before_tilt"` / `"after_tilt"` — `session_id`, `profile_name`
    /// - `"before_resource_start"` / `"after_resource_start"` — `resource_name`, `resource_kind`
    /// - `"after_ready"` — `session_id`, `profile_name`, `resource_count`, `startup_duration_secs`
    /// - `"before_stop"` / `"after_stop"` — `session_id`, `duration_secs`
    /// - `"on_error"` — `session_id`, `error`, `state`
    /// - `"on_report"` — `title`, `body`
    fn handle(&self, method: &str, params: &Value) -> Value;

    /// Called on shutdown. Override for cleanup.
    fn shutdown(&self) {}
}

/// Run a plugin. Reads JSON from stdin, dispatches to plugin methods,
/// writes JSON responses to stdout. Blocks until shutdown or EOF.
pub fn run<P: Plugin>() {
    let mut plugin = P::default();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) if !l.trim().is_empty() => l,
            _ => continue,
        };

        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let r = serde_json::json!({"status": "error", "message": format!("JSON parse error: {e}")});
                let _ = writeln!(stdout, "{r}");
                let _ = stdout.flush();
                continue;
            }
        };

        let method = msg["method"].as_str().unwrap_or("");

        let response = match method {
            "initialize" => {
                plugin.initialize(msg["config"].clone());
                serde_json::json!({"status": "ok"})
            }
            "shutdown" => {
                plugin.shutdown();
                let r = serde_json::json!({"status": "ok"});
                let _ = writeln!(stdout, "{r}");
                let _ = stdout.flush();
                break;
            }
            _ => plugin.handle(method, &msg),
        };

        let _ = writeln!(stdout, "{response}");
        let _ = stdout.flush();
    }
}

/// Helper: create an OK response.
pub fn ok() -> Value {
    serde_json::json!({"status": "ok"})
}

/// Helper: create an OK response with data.
pub fn ok_with_data(data: Value) -> Value {
    serde_json::json!({"status": "ok", "data": data})
}

/// Helper: create an error response.
pub fn error(message: &str) -> Value {
    serde_json::json!({"status": "error", "message": message})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct TestPlugin {
        initialized: bool,
    }

    impl Plugin for TestPlugin {
        fn name(&self) -> &str { "test" }
        fn initialize(&mut self, _config: Value) { self.initialized = true; }
        fn handle(&self, method: &str, _params: &Value) -> Value {
            match method {
                "ping" => ok_with_data(serde_json::json!("pong")),
                _ => error(&format!("unknown: {method}")),
            }
        }
    }

    #[test]
    fn helpers() {
        assert_eq!(ok()["status"], "ok");
        assert_eq!(error("fail")["status"], "error");
        assert_eq!(error("fail")["message"], "fail");
        assert_eq!(ok_with_data(serde_json::json!(42))["data"], 42);
    }
}
