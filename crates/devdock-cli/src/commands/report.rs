//! Bug report command implementation.

use anyhow::Result;
use devdock_diagnostics::BugReporter;

/// Run the bug reporting workflow interactively.
pub async fn report_bug() -> Result<()> {
    // Load diagnostics config from devdock.toml (sets DEVDOCK_JIRA_URL etc.)
    let project_config = if let Ok(cwd) = std::env::current_dir() {
        if let Ok((config, _)) = devdock_core::ProjectConfig::discover(&cwd) {
            config.export_diagnostics_env();
            Some(config)
        } else {
            None
        }
    } else {
        None
    };

    let reporter = BugReporter::new().interactive(true);

    match reporter.report().await {
        Ok(ticket_key) => {
            // HOOK: on_report — notify plugins about the report
            run_on_report_hook(project_config.as_ref(), &ticket_key);
            println!("\n✅ Bug report created successfully: {ticket_key}");
            Ok(())
        }
        Err(e) => {
            eprintln!("\n❌ Failed to create bug report: {e}");
            Err(e.into())
        }
    }
}

/// Run the bug reporting workflow with pre-filled details (non-interactive).
pub async fn report_bug_with_details(
    summary: String,
    description: String,
    command: String,
) -> Result<()> {
    // Load diagnostics config
    let project_config = if let Ok(cwd) = std::env::current_dir() {
        if let Ok((config, _)) = devdock_core::ProjectConfig::discover(&cwd) {
            config.export_diagnostics_env();
            Some(config)
        } else {
            None
        }
    } else {
        None
    };

    let reporter = BugReporter::with_details(summary.clone(), description.clone(), command.clone())
        .interactive(false);

    match reporter.report().await {
        Ok(ticket_key) => {
            // HOOK: on_report
            run_on_report_hook(project_config.as_ref(), &ticket_key);
            println!("\n✅ Bug report created successfully: {ticket_key}");
            Ok(())
        }
        Err(e) => {
            eprintln!("\n❌ Failed to create bug report: {e}");
            Err(e.into())
        }
    }
}

/// Fire on_report hook if plugins are configured.
fn run_on_report_hook(config: Option<&devdock_core::ProjectConfig>, ticket_key: &str) {
    let Some(config) = config else { return };
    if !config.plugins.hooks.has_any() {
        return;
    }

    let mut plugin_manager = devdock_core::PluginManager::new(config.plugins.hooks.clone());
    let plugin_config = serde_json::json!({
        "project": config.project.name,
        "workspace_root": config.project.workspace_root,
    });
    if plugin_manager.initialize_all(&plugin_config).is_err() {
        return;
    }

    let ctx = serde_json::json!({
        "ticket_key": ticket_key,
    });
    let results = plugin_manager.run_hook("on_report", &ctx);
    for (plugin_name, result) in &results {
        match result {
            Ok(_) => tracing::info!("[on_report] {plugin_name}: ok"),
            Err(e) => tracing::warn!("[on_report] {plugin_name}: {e}"),
        }
    }
    plugin_manager.shutdown_all();
}
