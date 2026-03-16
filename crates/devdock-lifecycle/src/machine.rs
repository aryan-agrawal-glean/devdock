//! The lifecycle state machine that drives DevDock startup and shutdown.
//!
//! `LifecycleMachine` holds the current state, fires hooks at transition points,
//! and delegates actual work (Docker, Tilt, etc.) to injected callbacks.

use crate::context::*;
use crate::hooks::{Hook, HookRunner, ResourceKind};
use crate::state::State;
use anyhow::{Context, Result};
use devdock_core::PluginManager;
use std::path::PathBuf;

/// Output callback for lifecycle messages (info, success, warn).
pub trait LifecycleOutput: Send {
    fn info(&self, msg: &str);
    fn success(&self, msg: &str);
    fn warn(&self, msg: &str);
}

/// Configuration for the lifecycle machine.
pub struct LifecycleConfig {
    pub profile_name: String,
    pub services: Option<String>,
    pub ci_mode: bool,
    pub run_monitor: bool,
    pub monitor_timeout: Option<u64>,
    pub telemetry_enabled: bool,
    pub dry_run: bool,
    pub workspace_root: PathBuf,
}

/// The lifecycle state machine.
///
/// Drives the DevDock startup/shutdown through well-defined states,
/// firing plugin hooks at each transition.
pub struct LifecycleMachine {
    state: State,
    config: LifecycleConfig,
    plugin_manager: PluginManager,
    output: Box<dyn LifecycleOutput>,
    command_start_time: chrono::DateTime<chrono::Utc>,
    instance_id: String,
    port_base: u16,
    platform: String,
    project_name: String,
}

impl LifecycleMachine {
    pub fn new(
        config: LifecycleConfig,
        plugin_manager: PluginManager,
        output: Box<dyn LifecycleOutput>,
    ) -> Self {
        Self {
            state: State::Initializing,
            config,
            plugin_manager,
            output,
            command_start_time: chrono::Utc::now(),
            instance_id: String::new(),
            port_base: 0,
            platform: String::new(),
            project_name: String::new(),
        }
    }

    /// Returns the current lifecycle state.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Set instance ID (after registration).
    pub fn set_instance_id(&mut self, id: String) {
        self.instance_id = id;
    }

    /// Set port base (after allocation).
    pub fn set_port_base(&mut self, port_base: u16) {
        self.port_base = port_base;
    }

    /// Set platform info from project config.
    pub fn set_platform(&mut self, platform: String) {
        self.platform = platform;
    }

    /// Set project name from project config.
    pub fn set_project_name(&mut self, name: String) {
        self.project_name = name;
    }

    /// Advance to the next state, returning the new state.
    pub fn advance(&mut self) -> Result<&State> {
        let next = self
            .state
            .next()
            .context(format!("Cannot advance from terminal state: {}", self.state))?;
        tracing::info!("Lifecycle: {} -> {}", self.state, next);
        self.state = next;
        Ok(&self.state)
    }

    /// Transition to Failed state.
    pub fn fail(&mut self, error: &str) {
        tracing::error!("Lifecycle: {} -> Failed: {}", self.state, error);
        let previous_state = self.state.to_string();
        self.state = State::Failed;

        // Fire on_error hook
        let ctx = serde_json::to_value(ErrorContext {
            session_id: self.instance_id.clone(),
            error: error.to_string(),
            state: previous_state,
            details: std::collections::HashMap::new(),
        })
        .unwrap_or_default();

        let mut runner = HookRunner::new(&mut self.plugin_manager);
        let results = runner.fire(&Hook::OnError, &ctx);
        log_hook_results(&*self.output, "on_error", &results);
    }

    /// Fire the hook associated with the current state transition.
    pub fn fire_hook(&mut self, hook: Hook, context: &serde_json::Value) {
        let hook_name = hook.to_string();
        let mut runner = HookRunner::new(&mut self.plugin_manager);
        let results = runner.fire(&hook, context);
        log_hook_results(&*self.output, &hook_name, &results);
    }

    /// Fire the after_profile_loaded hook.
    pub fn fire_after_profile_loaded(&mut self) {
        let ctx = serde_json::to_value(ProfileContext {
            profile_name: self.config.profile_name.clone(),
            workspace_root: self.config.workspace_root.to_string_lossy().to_string(),
            platform: self.platform.clone(),
            project_name: self.project_name.clone(),
        })
        .unwrap_or_default();
        self.fire_hook(Hook::AfterProfileLoaded, &ctx);
    }

    /// Fire the before_infra_ready hook.
    pub fn fire_before_infra_ready(&mut self) {
        let ctx = serde_json::to_value(InfraContext {
            profile_name: self.config.profile_name.clone(),
            workspace_root: self.config.workspace_root.to_string_lossy().to_string(),
            platform: self.platform.clone(),
            instance_id: self.instance_id.clone(),
            port_base: self.port_base,
        })
        .unwrap_or_default();
        self.fire_hook(Hook::BeforeInfraReady, &ctx);
    }

    /// Fire the before_config_generated hook.
    pub fn fire_before_config_generated(&mut self) {
        let runtime_dir = self.config.workspace_root.join("devdock/runtime_generated");
        let ctx = serde_json::to_value(ConfigContext {
            profile_name: self.config.profile_name.clone(),
            workspace_root: self.config.workspace_root.to_string_lossy().to_string(),
            runtime_dir: runtime_dir.to_string_lossy().to_string(),
            platform: self.platform.clone(),
            instance_id: self.instance_id.clone(),
        })
        .unwrap_or_default();
        self.fire_hook(Hook::BeforeConfigGenerated, &ctx);
    }

    /// Fire the after_config_generated hook.
    pub fn fire_after_config_generated(&mut self) {
        let runtime_dir = self.config.workspace_root.join("devdock/runtime_generated");
        let ctx = serde_json::to_value(ConfigContext {
            profile_name: self.config.profile_name.clone(),
            workspace_root: self.config.workspace_root.to_string_lossy().to_string(),
            runtime_dir: runtime_dir.to_string_lossy().to_string(),
            platform: self.platform.clone(),
            instance_id: self.instance_id.clone(),
        })
        .unwrap_or_default();
        self.fire_hook(Hook::AfterConfigGenerated, &ctx);
    }

    /// Fire the before_tilt hook.
    pub fn fire_before_tilt(&mut self) {
        let ctx = serde_json::to_value(TiltContext {
            profile_name: self.config.profile_name.clone(),
            session_id: self.instance_id.clone(),
            workspace_root: self.config.workspace_root.to_string_lossy().to_string(),
        })
        .unwrap_or_default();
        self.fire_hook(Hook::BeforeTilt, &ctx);
    }

    /// Fire the after_tilt hook.
    pub fn fire_after_tilt(&mut self) {
        let ctx = serde_json::to_value(TiltContext {
            profile_name: self.config.profile_name.clone(),
            session_id: self.instance_id.clone(),
            workspace_root: self.config.workspace_root.to_string_lossy().to_string(),
        })
        .unwrap_or_default();
        self.fire_hook(Hook::AfterTilt, &ctx);
    }

    /// Fire per-resource hooks.
    pub fn fire_before_resource_start(&mut self, name: &str, kind: ResourceKind) {
        let ctx = serde_json::to_value(ResourceContext {
            resource_name: name.to_string(),
            resource_kind: kind.clone(),
            session_id: self.instance_id.clone(),
            workspace_root: self.config.workspace_root.to_string_lossy().to_string(),
            config: serde_json::json!({}),
        })
        .unwrap_or_default();
        self.fire_hook(Hook::BeforeResourceStart { name: name.to_string(), kind }, &ctx);
    }

    pub fn fire_after_resource_start(&mut self, name: &str, kind: ResourceKind) {
        let ctx = serde_json::to_value(ResourceContext {
            resource_name: name.to_string(),
            resource_kind: kind.clone(),
            session_id: self.instance_id.clone(),
            workspace_root: self.config.workspace_root.to_string_lossy().to_string(),
            config: serde_json::json!({}),
        })
        .unwrap_or_default();
        self.fire_hook(Hook::AfterResourceStart { name: name.to_string(), kind }, &ctx);
    }

    /// Fire the after_ready hook.
    pub fn fire_after_ready(&mut self, resource_count: usize, startup_duration_secs: f64) {
        let ctx = serde_json::to_value(ReadyContext {
            session_id: self.instance_id.clone(),
            profile_name: self.config.profile_name.clone(),
            resource_count,
            startup_duration_secs,
        })
        .unwrap_or_default();
        self.fire_hook(Hook::AfterReady, &ctx);
    }

    /// Fire the before_stop hook.
    pub fn fire_before_stop(&mut self) {
        let duration = (chrono::Utc::now() - self.command_start_time).num_seconds();
        let ctx = serde_json::to_value(StopContext {
            session_id: self.instance_id.clone(),
            duration_secs: duration,
        })
        .unwrap_or_default();
        self.fire_hook(Hook::BeforeStop, &ctx);
    }

    /// Fire the after_stop hook.
    pub fn fire_after_stop(&mut self) {
        let duration = (chrono::Utc::now() - self.command_start_time).num_seconds();
        let ctx = serde_json::to_value(StopContext {
            session_id: self.instance_id.clone(),
            duration_secs: duration,
        })
        .unwrap_or_default();
        self.fire_hook(Hook::AfterStop, &ctx);
    }

    /// Shutdown all plugins.
    pub fn shutdown_plugins(&mut self) {
        self.plugin_manager.shutdown_all();
    }

    /// Get the command start time.
    pub fn command_start_time(&self) -> chrono::DateTime<chrono::Utc> {
        self.command_start_time
    }

    /// Get the instance ID.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Get the lifecycle config.
    pub fn config(&self) -> &LifecycleConfig {
        &self.config
    }

    /// Get a mutable reference to the plugin manager.
    pub fn plugin_manager_mut(&mut self) -> &mut PluginManager {
        &mut self.plugin_manager
    }

    /// Check if the plugin manager has plugins.
    pub fn has_plugins(&self) -> bool {
        self.plugin_manager.has_plugins()
    }

    /// Get the output interface.
    pub fn output(&self) -> &dyn LifecycleOutput {
        &*self.output
    }
}

/// Log hook results.
fn log_hook_results(
    output: &dyn LifecycleOutput,
    hook: &str,
    results: &[(String, Result<serde_json::Value, String>)],
) {
    for (plugin_name, result) in results {
        match result {
            Ok(response) => {
                let status = response.get("status").and_then(|s| s.as_str()).unwrap_or("unknown");
                if status == "ok" {
                    output.info(&format!("[{hook}] {plugin_name}: ok"));
                } else {
                    let msg =
                        response.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error");
                    output.warn(&format!("[{hook}] {plugin_name}: {status} — {msg}"));
                }
            }
            Err(e) => {
                output.warn(&format!("[{hook}] {plugin_name}: error — {e}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestOutput;
    impl LifecycleOutput for TestOutput {
        fn info(&self, _msg: &str) {}
        fn success(&self, _msg: &str) {}
        fn warn(&self, _msg: &str) {}
    }

    #[test]
    fn machine_starts_in_initializing() {
        let config = LifecycleConfig {
            profile_name: "test".into(),
            services: None,
            ci_mode: false,
            run_monitor: false,
            monitor_timeout: None,
            telemetry_enabled: false,
            dry_run: false,
            workspace_root: PathBuf::from("/tmp"),
        };
        let machine = LifecycleMachine::new(config, PluginManager::empty(), Box::new(TestOutput));
        assert_eq!(*machine.state(), State::Initializing);
    }

    #[test]
    fn machine_advances_through_states() {
        let config = LifecycleConfig {
            profile_name: "test".into(),
            services: None,
            ci_mode: false,
            run_monitor: false,
            monitor_timeout: None,
            telemetry_enabled: false,
            dry_run: false,
            workspace_root: PathBuf::from("/tmp"),
        };
        let mut machine =
            LifecycleMachine::new(config, PluginManager::empty(), Box::new(TestOutput));

        machine.advance().unwrap();
        assert_eq!(*machine.state(), State::ProfileLoaded);

        machine.advance().unwrap();
        assert_eq!(*machine.state(), State::PluginsReady);
    }

    #[test]
    fn machine_fails_correctly() {
        let config = LifecycleConfig {
            profile_name: "test".into(),
            services: None,
            ci_mode: false,
            run_monitor: false,
            monitor_timeout: None,
            telemetry_enabled: false,
            dry_run: false,
            workspace_root: PathBuf::from("/tmp"),
        };
        let mut machine =
            LifecycleMachine::new(config, PluginManager::empty(), Box::new(TestOutput));

        machine.fail("test error");
        assert_eq!(*machine.state(), State::Failed);
    }
}
