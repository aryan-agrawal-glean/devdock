//! Plugin hook definitions and dispatch.

use devdock_core::PluginManager;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Kind of resource (infra or service).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Infra,
    Service,
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Infra => write!(f, "infra"),
            Self::Service => write!(f, "service"),
        }
    }
}

/// Lifecycle hooks fired at state transitions.
#[derive(Debug, Clone)]
pub enum Hook {
    AfterProfileLoaded,
    BeforeInfraReady,
    AfterInfraReady,
    BeforeConfigGenerated,
    AfterConfigGenerated,
    BeforeTilt,
    AfterTilt,
    BeforeResourceStart { name: String, kind: ResourceKind },
    AfterResourceStart { name: String, kind: ResourceKind },
    AfterReady,
    BeforeStop,
    AfterStop,
    OnError,
    OnReport,
}

impl Hook {
    /// Returns the hook name used in devdock.toml and plugin dispatch.
    pub fn method_name(&self) -> &str {
        match self {
            Self::AfterProfileLoaded => "after_profile_loaded",
            Self::BeforeInfraReady => "before_infra_ready",
            Self::AfterInfraReady => "after_infra_ready",
            Self::BeforeConfigGenerated => "before_config_generated",
            Self::AfterConfigGenerated => "after_config_generated",
            Self::BeforeTilt => "before_tilt",
            Self::AfterTilt => "after_tilt",
            Self::BeforeResourceStart { .. } => "before_resource_start",
            Self::AfterResourceStart { .. } => "after_resource_start",
            Self::AfterReady => "after_ready",
            Self::BeforeStop => "before_stop",
            Self::AfterStop => "after_stop",
            Self::OnError => "on_error",
            Self::OnReport => "on_report",
        }
    }
}

impl fmt::Display for Hook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BeforeResourceStart { name, kind } => {
                write!(f, "before_resource_start({name}, {kind})")
            }
            Self::AfterResourceStart { name, kind } => {
                write!(f, "after_resource_start({name}, {kind})")
            }
            _ => write!(f, "{}", self.method_name()),
        }
    }
}

/// Runs hooks by dispatching to the PluginManager.
pub struct HookRunner<'a> {
    plugin_manager: &'a mut PluginManager,
}

impl<'a> HookRunner<'a> {
    pub fn new(plugin_manager: &'a mut PluginManager) -> Self {
        Self { plugin_manager }
    }

    /// Fire a lifecycle hook, dispatching to all configured plugins.
    ///
    /// For resource hooks, the context is enriched with resource name and kind.
    /// Returns hook results from each plugin.
    pub fn fire(
        &mut self,
        hook: &Hook,
        context: &serde_json::Value,
    ) -> Vec<(String, Result<serde_json::Value, String>)> {
        if !self.plugin_manager.has_plugins() {
            return Vec::new();
        }

        match hook {
            Hook::BeforeResourceStart { name, kind } | Hook::AfterResourceStart { name, kind } => {
                let mut enriched = context.clone();
                if let Some(obj) = enriched.as_object_mut() {
                    obj.insert("resource_name".to_string(), serde_json::json!(name));
                    obj.insert("resource_kind".to_string(), serde_json::json!(kind.to_string()));
                }
                self.plugin_manager.run_resource_hook(hook.method_name(), name, &enriched)
            }
            _ => self.plugin_manager.run_hook(hook.method_name(), context),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_method_names() {
        assert_eq!(Hook::AfterProfileLoaded.method_name(), "after_profile_loaded");
        assert_eq!(Hook::BeforeInfraReady.method_name(), "before_infra_ready");
        assert_eq!(Hook::AfterTilt.method_name(), "after_tilt");
        assert_eq!(Hook::OnError.method_name(), "on_error");
        assert_eq!(Hook::OnReport.method_name(), "on_report");
    }

    #[test]
    fn resource_hook_display() {
        let hook = Hook::AfterResourceStart { name: "sql".into(), kind: ResourceKind::Infra };
        assert_eq!(hook.to_string(), "after_resource_start(sql, infra)");
    }
}
