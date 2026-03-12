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
pub trait Plugin: Default {
    /// Plugin name (for logging).
    fn name(&self) -> &str;

    /// Called once with config from devdock.toml.
    fn initialize(&mut self, config: Value);

    /// Handle a method call. Common methods:
    /// - `"emit"` — telemetry event (params has `"event"` field)
    /// - `"fetch"` — config storage request (params has `"key"` field)
    /// - `"report"` — bug report (params has `"title"`, `"body"` fields)
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
