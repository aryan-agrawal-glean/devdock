//! Plugin lifecycle manager — spawns, communicates with, and shuts down hook plugins.
//!
//! Plugins are external processes that communicate via JSON-lines over stdin/stdout.
//! The PluginManager owns all plugin processes for a session and dispatches
//! lifecycle hooks to them in the order configured in `devdock.toml`.

use crate::project_config::{PluginHooks, ResourceHooks};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

/// A running plugin process.
struct PluginProcess {
    name: String,
    child: Child,
}

impl PluginProcess {
    /// Send a JSON message and read the JSON response.
    fn send(&mut self, msg: &serde_json::Value) -> Result<serde_json::Value, String> {
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| format!("Plugin '{}': stdin not available", self.name))?;

        let msg_str = serde_json::to_string(msg)
            .map_err(|e| format!("Plugin '{}': JSON serialize: {e}", self.name))?;

        writeln!(stdin, "{msg_str}")
            .map_err(|e| format!("Plugin '{}': write failed: {e}", self.name))?;
        stdin.flush().map_err(|e| format!("Plugin '{}': flush failed: {e}", self.name))?;

        let stdout = self
            .child
            .stdout
            .as_mut()
            .ok_or_else(|| format!("Plugin '{}': stdout not available", self.name))?;

        let mut response = String::new();
        let mut reader = BufReader::new(stdout);
        reader
            .read_line(&mut response)
            .map_err(|e| format!("Plugin '{}': read failed: {e}", self.name))?;

        if response.trim().is_empty() {
            return Err(format!(
                "Plugin '{}': empty response (process may have exited)",
                self.name
            ));
        }

        serde_json::from_str(&response)
            .map_err(|e| format!("Plugin '{}': JSON parse response: {e}", self.name))
    }

    /// Send shutdown message (best-effort) and wait for process exit.
    fn shutdown(&mut self) {
        let msg = serde_json::json!({"method": "shutdown"});
        let _ = self.send(&msg);
        let _ = self.child.wait();
    }
}

/// Manages all plugin processes for a DevDock session.
///
/// Plugins are spawned once at session start and reused across hook invocations.
/// Each hook dispatches to the configured plugins in order.
pub struct PluginManager {
    /// All running plugin processes, keyed by plugin name.
    processes: HashMap<String, PluginProcess>,
    /// Hook configuration — which plugins run at which hook point.
    hooks: PluginHooks,
    /// Per-resource hook configuration.
    resource_hooks: HashMap<String, ResourceHooks>,
}

impl PluginManager {
    /// Create a PluginManager from project config hooks.
    ///
    /// Does NOT spawn any processes yet — call `initialize_all()` to start them.
    pub fn new(hooks: PluginHooks) -> Self {
        Self { processes: HashMap::new(), hooks, resource_hooks: HashMap::new() }
    }

    /// Create a PluginManager with resource hooks.
    pub fn with_resource_hooks(
        hooks: PluginHooks,
        resource_hooks: HashMap<String, ResourceHooks>,
    ) -> Self {
        Self { processes: HashMap::new(), hooks, resource_hooks }
    }

    /// Create a no-op PluginManager (no hooks configured).
    pub fn empty() -> Self {
        Self {
            processes: HashMap::new(),
            hooks: PluginHooks::default(),
            resource_hooks: HashMap::new(),
        }
    }

    /// Returns true if any hooks are configured.
    pub fn has_plugins(&self) -> bool {
        self.hooks.has_any()
            || self
                .resource_hooks
                .values()
                .any(|rh| !rh.before_start.is_empty() || !rh.after_start.is_empty())
    }

    /// Spawn all referenced plugin processes and send initialize message.
    ///
    /// The `config` value is sent as the initialize payload to each plugin.
    pub fn initialize_all(&mut self, config: &serde_json::Value) -> crate::Result<()> {
        let mut all_plugins = self.hooks.all_plugins();

        // Also include plugins from resource hooks
        let mut seen: std::collections::HashSet<String> = all_plugins.iter().cloned().collect();
        for rh in self.resource_hooks.values() {
            for name in rh.before_start.iter().chain(rh.after_start.iter()) {
                if seen.insert(name.clone()) {
                    all_plugins.push(name.clone());
                }
            }
        }

        if all_plugins.is_empty() {
            return Ok(());
        }

        for plugin_name in &all_plugins {
            // Resolve plugin executable path
            let plugin_path = resolve_plugin_path(plugin_name)?;

            // Spawn process
            let child = Command::new(&plugin_path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|e| {
                    crate::Error::PluginError(format!(
                        "Failed to start plugin '{plugin_name}' at '{plugin_path}': {e}"
                    ))
                })?;

            let mut process = PluginProcess { name: plugin_name.clone(), child };

            // Send initialize
            let init_msg = serde_json::json!({
                "method": "initialize",
                "config": config,
            });

            let response = process.send(&init_msg).map_err(|e| {
                crate::Error::PluginError(format!("Plugin '{plugin_name}' initialize failed: {e}"))
            })?;

            if response.get("status").and_then(|s| s.as_str()) != Some("ok") {
                return Err(crate::Error::PluginError(format!(
                    "Plugin '{plugin_name}' initialization rejected: {response}"
                )));
            }

            self.processes.insert(plugin_name.clone(), process);
        }

        Ok(())
    }

    /// Run a lifecycle hook, dispatching to all configured plugins in order.
    ///
    /// Returns the list of responses from each plugin.
    /// Non-fatal: if a plugin returns an error status, it's included in the result
    /// but doesn't stop other plugins from running.
    pub fn run_hook(
        &mut self,
        hook: &str,
        context: &serde_json::Value,
    ) -> Vec<(String, Result<serde_json::Value, String>)> {
        let plugin_names: Vec<String> = self.hooks.get_hook(hook).to_vec();
        let mut results = Vec::new();

        for plugin_name in &plugin_names {
            let msg = serde_json::json!({
                "method": hook,
                "params": context,
            });

            let result = if let Some(process) = self.processes.get_mut(plugin_name) {
                process.send(&msg)
            } else {
                Err(format!("Plugin '{plugin_name}' not initialized"))
            };

            results.push((plugin_name.clone(), result));
        }

        results
    }

    /// Run a per-resource hook, dispatching to plugins configured for that resource.
    ///
    /// Looks up resource_hooks in the PluginsSection to find which plugins handle
    /// before_start / after_start for the given resource name.
    pub fn run_resource_hook(
        &mut self,
        hook: &str,
        resource_name: &str,
        context: &serde_json::Value,
    ) -> Vec<(String, Result<serde_json::Value, String>)> {
        let plugin_names: Vec<String> = self
            .resource_hooks
            .get(resource_name)
            .map(|rh| match hook {
                "before_resource_start" => rh.before_start.clone(),
                "after_resource_start" => rh.after_start.clone(),
                _ => Vec::new(),
            })
            .unwrap_or_default();

        let mut results = Vec::new();
        for plugin_name in &plugin_names {
            let msg = serde_json::json!({
                "method": hook,
                "params": context,
            });

            let result = if let Some(process) = self.processes.get_mut(plugin_name) {
                process.send(&msg)
            } else {
                Err(format!("Plugin '{plugin_name}' not initialized"))
            };

            results.push((plugin_name.clone(), result));
        }
        results
    }

    /// Shutdown all plugin processes gracefully.
    pub fn shutdown_all(&mut self) {
        for (_, mut process) in self.processes.drain() {
            process.shutdown();
        }
    }
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

/// Resolve a plugin name to an executable path.
///
/// Search order:
/// 1. If the name is an absolute path, use it directly
/// 2. Look in ~/.devdock/plugins/
/// 3. Look in $PATH (via `which`)
fn resolve_plugin_path(name: &str) -> crate::Result<String> {
    // Absolute path
    if std::path::Path::new(name).is_absolute() {
        if std::path::Path::new(name).exists() {
            return Ok(name.to_string());
        }
        return Err(crate::Error::PluginError(format!("Plugin not found at path: {name}")));
    }

    // Check ~/.devdock/plugins/
    if let Ok(home) = std::env::var("HOME") {
        let plugin_dir = std::path::PathBuf::from(home).join(".devdock/plugins");
        let plugin_path = plugin_dir.join(name);
        if plugin_path.exists() {
            return Ok(plugin_path.to_string_lossy().to_string());
        }
    }

    // Check $PATH
    if let Ok(path) = which::which(name) {
        return Ok(path.to_string_lossy().to_string());
    }

    Err(crate::Error::PluginError(format!(
        "Plugin '{name}' not found. Install it to ~/.devdock/plugins/ or ensure it's in $PATH."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_manager_has_no_plugins() {
        let mgr = PluginManager::empty();
        assert!(!mgr.has_plugins());
    }

    #[test]
    fn manager_with_hooks_has_plugins() {
        let hooks =
            PluginHooks { before_infra_ready: vec!["test-plugin".into()], ..Default::default() };
        let mgr = PluginManager::new(hooks);
        assert!(mgr.has_plugins());
    }

    #[test]
    fn run_hook_on_empty_returns_empty() {
        let mut mgr = PluginManager::empty();
        let results = mgr.run_hook("before_infra_ready", &serde_json::json!({}));
        assert!(results.is_empty());
    }

    #[test]
    fn resolve_nonexistent_plugin_errors() {
        let result = resolve_plugin_path("nonexistent-devdock-plugin-xyz-12345");
        assert!(result.is_err());
    }
}
