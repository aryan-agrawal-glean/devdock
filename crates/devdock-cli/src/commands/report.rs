//! Bug report command implementation.

use anyhow::Result;
use devdock_diagnostics::BugReporter;

/// Run the bug reporting workflow interactively.
pub async fn report_bug() -> Result<()> {
    // Load diagnostics config from devdock.toml (sets DEVDOCK_JIRA_URL etc.)
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok((config, _)) = devdock_core::ProjectConfig::discover(&cwd) {
            config.export_diagnostics_env();
        }
    }

    let reporter = BugReporter::new().interactive(true);

    match reporter.report().await {
        Ok(ticket_key) => {
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
    let reporter = BugReporter::with_details(summary, description, command).interactive(false);

    match reporter.report().await {
        Ok(ticket_key) => {
            println!("\n✅ Bug report created successfully: {ticket_key}");
            Ok(())
        }
        Err(e) => {
            eprintln!("\n❌ Failed to create bug report: {e}");
            Err(e.into())
        }
    }
}
