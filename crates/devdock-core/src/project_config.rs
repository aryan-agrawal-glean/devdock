//! Project configuration (`devdock.toml`).
//!
//! Per-project configuration that replaces hardcoded values.
//! Located at `.devdock/devdock.toml` in the repo root.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Top-level project configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub project: ProjectSection,
    #[serde(default)]
    pub registry: RegistrySection,
    #[serde(default)]
    pub remote: RemoteSection,
    #[serde(default)]
    pub telemetry: TelemetrySection,
    #[serde(default)]
    pub cloud: CloudSection,
    #[serde(default)]
    pub diagnostics: DiagnosticsSection,
    #[serde(default)]
    pub plugins: PluginsSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSection {
    pub name: String,
    #[serde(default = "default_domain")]
    pub domain: String,
    pub workspace_root: Option<String>,
    #[serde(default = "default_services_dir")]
    pub services_dir: String,
    #[serde(default = "default_infra_dir")]
    pub infra_dir: String,
    #[serde(default = "default_hooks_dir")]
    pub hooks_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RegistrySection {
    #[serde(default = "default_registry")]
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSection {
    #[serde(default = "default_remote_provider")]
    pub provider: String,
    pub template: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_telemetry_backend")]
    pub backend: String,
    pub project: Option<String>,
    pub dataset: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudSection {
    #[serde(default = "default_platform")]
    pub platform: String,
    pub project: Option<String>,
    pub region: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsSection {
    #[serde(default = "default_diagnostics_reporter")]
    pub reporter: String,
    /// Jira instance URL (e.g., "https://your-org.atlassian.net")
    pub jira_url: Option<String>,
    /// Jira service account email
    pub jira_email: Option<String>,
    /// Jira project key (e.g., "ENG")
    pub jira_project: Option<String>,
    /// GCP project for Secret Manager (to fetch Jira/Slack tokens)
    pub secret_project: Option<String>,
    /// Secret name for Jira API token
    pub jira_secret: Option<String>,
    /// Slack channel for notifications
    pub slack_channel: Option<String>,
    /// Secret name for Slack bot token
    pub slack_secret: Option<String>,
    /// Jira parent issue key (tickets created under this epic)
    pub jira_parent_issue: Option<String>,
}

/// Plugin configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginsSection {
    /// Lifecycle hooks — maps hook names to ordered lists of plugin executables.
    #[serde(default)]
    pub hooks: PluginHooks,
}

/// Lifecycle hook definitions.
///
/// Each hook maps to an ordered list of plugin executable names.
/// Plugins are spawned as child processes and communicate via JSON-lines.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginHooks {
    /// Runs before anything — auth, custom infra provisioning.
    #[serde(default)]
    pub pre_start: Vec<String>,
    /// Runs after generic config gen, before Tilt — enrich configs, generate devdock.ini.
    #[serde(default)]
    pub pre_tilt: Vec<String>,
    /// Runs after Tilt starts — telemetry, notifications.
    #[serde(default)]
    pub post_start: Vec<String>,
    /// Runs when `devdock report` is invoked.
    #[serde(default)]
    pub on_report: Vec<String>,
    /// Runs when `devdock stop` is invoked — cleanup, final telemetry.
    #[serde(default)]
    pub on_stop: Vec<String>,
}

impl PluginHooks {
    /// Returns all unique plugin names referenced across all hooks.
    pub fn all_plugins(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut plugins = Vec::new();
        for list in
            [&self.pre_start, &self.pre_tilt, &self.post_start, &self.on_report, &self.on_stop]
        {
            for name in list {
                if seen.insert(name.clone()) {
                    plugins.push(name.clone());
                }
            }
        }
        plugins
    }

    /// Returns the plugin names for a given hook.
    pub fn get_hook(&self, hook: &str) -> &[String] {
        match hook {
            "pre_start" => &self.pre_start,
            "pre_tilt" => &self.pre_tilt,
            "post_start" => &self.post_start,
            "on_report" => &self.on_report,
            "on_stop" => &self.on_stop,
            _ => &[],
        }
    }

    /// Returns true if any hooks are configured.
    pub fn has_any(&self) -> bool {
        !self.pre_start.is_empty()
            || !self.pre_tilt.is_empty()
            || !self.post_start.is_empty()
            || !self.on_report.is_empty()
            || !self.on_stop.is_empty()
    }
}

fn default_domain() -> String {
    "localhost".into()
}
fn default_services_dir() -> String {
    "devdock/services".into()
}
fn default_infra_dir() -> String {
    "devdock/infra".into()
}
fn default_hooks_dir() -> String {
    "devdock/hooks".into()
}
fn default_registry() -> String {
    "docker.io/library".into()
}
fn default_remote_provider() -> String {
    "ssh".into()
}
fn default_telemetry_backend() -> String {
    "stdout".into()
}
fn default_platform() -> String {
    "none".into()
}
fn default_diagnostics_reporter() -> String {
    "stdout".into()
}

impl Default for RemoteSection {
    fn default() -> Self {
        Self { provider: default_remote_provider(), template: None }
    }
}
impl Default for TelemetrySection {
    fn default() -> Self {
        Self { enabled: false, backend: default_telemetry_backend(), project: None, dataset: None }
    }
}
impl Default for CloudSection {
    fn default() -> Self {
        Self { platform: default_platform(), project: None, region: None }
    }
}
impl Default for DiagnosticsSection {
    fn default() -> Self {
        Self {
            reporter: default_diagnostics_reporter(),
            jira_url: None,
            jira_email: None,
            jira_project: None,
            secret_project: None,
            jira_secret: None,
            slack_channel: None,
            slack_secret: None,
            jira_parent_issue: None,
        }
    }
}

impl ProjectConfig {
    /// Export diagnostics config as environment variables so jira.rs/slack.rs can read them.
    pub fn export_diagnostics_env(&self) {
        if let Some(ref url) = self.diagnostics.jira_url {
            std::env::set_var("DEVDOCK_JIRA_URL", url);
        }
        if let Some(ref email) = self.diagnostics.jira_email {
            std::env::set_var("DEVDOCK_JIRA_EMAIL", email);
        }
        if let Some(ref project) = self.diagnostics.jira_project {
            std::env::set_var("DEVDOCK_JIRA_PROJECT", project);
        }
        if let Some(ref project) = self.diagnostics.secret_project {
            std::env::set_var("DEVDOCK_SECRET_PROJECT", project);
        }
        if let Some(ref secret) = self.diagnostics.jira_secret {
            std::env::set_var("DEVDOCK_JIRA_SECRET", secret);
        }
        if let Some(ref channel) = self.diagnostics.slack_channel {
            std::env::set_var("DEVDOCK_SLACK_CHANNEL", channel);
        }
        if let Some(ref secret) = self.diagnostics.slack_secret {
            std::env::set_var("DEVDOCK_SLACK_SECRET", secret);
        }
        if let Some(ref issue) = self.diagnostics.jira_parent_issue {
            std::env::set_var("DEVDOCK_JIRA_PARENT_ISSUE", issue);
        }
    }

    pub fn load(path: &Path) -> crate::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::Error::Config(format!("Failed to read {}: {e}", path.display())))?;
        Self::parse(&content)
    }

    pub fn parse(content: &str) -> crate::Result<Self> {
        toml::from_str(content)
            .map_err(|e| crate::Error::Config(format!("Invalid devdock.toml: {e}")))
    }

    /// Walk up from `start_dir` looking for `.devdock/devdock.toml`.
    pub fn discover(start_dir: &Path) -> crate::Result<(Self, PathBuf)> {
        let mut dir = start_dir.to_path_buf();
        loop {
            let config_path = dir.join(".devdock/devdock.toml");
            if config_path.exists() {
                let config = Self::load(&config_path)?;
                return Ok((config, dir));
            }
            if !dir.pop() {
                return Err(crate::Error::Config(
                    "No .devdock/devdock.toml found. Run `devdock init` to create one.".into(),
                ));
            }
        }
    }

    pub fn workspace_root(&self) -> String {
        self.project
            .workspace_root
            .clone()
            .unwrap_or_else(|| format!("/home/workspace/{}", self.project.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal() {
        let c = ProjectConfig::parse("[project]\nname = \"test\"").unwrap();
        assert_eq!(c.project.name, "test");
        assert_eq!(c.project.domain, "localhost");
        assert_eq!(c.project.services_dir, "devdock/services");
        assert_eq!(c.project.infra_dir, "devdock/infra");
        assert_eq!(c.remote.provider, "ssh");
        assert!(!c.telemetry.enabled);
        assert_eq!(c.cloud.platform, "none");
    }

    #[test]
    fn parse_full() {
        let toml = r#"
        [project]
        name = "scio"
        domain = "local.myapp.com"
        services_dir = "devdock/services"
        infra_dir = "devdock/infra"

        [cloud]
        platform = "google"
        project = "test-gcp-project"

        [telemetry]
        enabled = true
        backend = "bigquery"

        [remote]
        provider = "coder"
        template = "devdock-remote-template"

        [diagnostics]
        reporter = "jira"
        "#;
        let c = ProjectConfig::parse(toml).unwrap();
        assert_eq!(c.project.name, "scio");
        assert_eq!(c.cloud.platform, "google");
        assert_eq!(c.telemetry.backend, "bigquery");
        assert_eq!(c.remote.provider, "coder");
        assert_eq!(c.diagnostics.reporter, "jira");
    }

    #[test]
    fn parse_with_plugin_hooks() {
        let toml = r#"
        [project]
        name = "test"

        [plugins.hooks]
        pre_start = ["devdock-plugin-gcloud-auth"]
        pre_tilt = ["devdock-plugin-config-gen"]
        post_start = ["devdock-plugin-bigquery-sync"]
        on_report = ["devdock-plugin-jira-reporter", "devdock-plugin-slack-notifier"]
        on_stop = ["devdock-plugin-bigquery-sync"]
        "#;
        let c = ProjectConfig::parse(toml).unwrap();
        assert_eq!(c.plugins.hooks.pre_start, vec!["devdock-plugin-gcloud-auth"]);
        assert_eq!(c.plugins.hooks.pre_tilt, vec!["devdock-plugin-config-gen"]);
        assert_eq!(c.plugins.hooks.on_report.len(), 2);
        assert!(c.plugins.hooks.has_any());
        // all_plugins deduplicates bigquery-sync (in post_start and on_stop)
        let all = c.plugins.hooks.all_plugins();
        assert_eq!(all.iter().filter(|p| *p == "devdock-plugin-bigquery-sync").count(), 1);
    }

    #[test]
    fn parse_no_plugins_defaults_empty() {
        let c = ProjectConfig::parse("[project]\nname = \"test\"").unwrap();
        assert!(!c.plugins.hooks.has_any());
        assert!(c.plugins.hooks.all_plugins().is_empty());
    }

    #[test]
    fn missing_project_errors() {
        assert!(ProjectConfig::parse("[registry]\nurl = \"x\"").is_err());
    }

    #[test]
    fn invalid_toml_errors() {
        assert!(ProjectConfig::parse("not valid!!!").is_err());
    }

    #[test]
    fn workspace_root_default() {
        let c = ProjectConfig::parse("[project]\nname = \"app\"").unwrap();
        assert_eq!(c.workspace_root(), "/home/workspace/app");
    }

    #[test]
    fn load_from_fixture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("tests/fixtures/minimal-python/.devdock/devdock.toml");
        if path.exists() {
            let c = ProjectConfig::load(&path).unwrap();
            assert_eq!(c.project.name, "minimal-python");
        }
    }
}
