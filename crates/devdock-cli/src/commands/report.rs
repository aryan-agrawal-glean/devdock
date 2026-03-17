//! Bug report command implementation.
//!
//! For remote devdock instances, diagnostics (Tilt snapshot, system info, Docker
//! containers, config files) live on the remote workspace. We detect remote mode
//! via the state DB, collect interactive input locally, then delegate the actual
//! report to the remote via `coder ssh`.

use anyhow::{Context, Result};
use colored::Colorize;
use devdock_diagnostics::BugReporter;
use devdock_state::StateManager;
use dialoguer::{Confirm, Input};
use std::process::Command;

/// Run the bug reporting workflow interactively.
pub async fn report_bug() -> Result<()> {
    if let Some(workspace_name) = get_remote_workspace().await? {
        return report_via_remote_interactive(&workspace_name).await;
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
    if let Some(workspace_name) = get_remote_workspace().await? {
        return report_via_remote(&workspace_name, &summary, &description, &command).await;
    }

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

/// Return the coder workspace name when the current directory belongs to a
/// remote devdock instance, `None` otherwise.
async fn get_remote_workspace() -> Result<Option<String>> {
    let state_manager = match StateManager::new().await {
        Ok(sm) => sm,
        Err(_) => return Ok(None),
    };

    let workspace_path = std::env::current_dir()?.display().to_string();
    let instance = match state_manager.get_instance_by_workspace(&workspace_path).await? {
        Some(i) if i.mode == "remote" => i,
        _ => return Ok(None),
    };

    if let Some(ref name) = instance.workspace_name {
        return Ok(Some(name.clone()));
    }

    if let Ok(Some((workspace_name, _, _, _))) = state_manager.get_sync_session(&instance.id).await
    {
        return Ok(Some(workspace_name));
    }

    Ok(None)
}

/// Collect user input locally, then forward the report to the remote workspace.
async fn report_via_remote_interactive(workspace_name: &str) -> Result<()> {
    println!("{}", "\n🐛 DevDock Bug Report (Remote)".bold().cyan());
    println!("{}", "=".repeat(50).cyan());
    println!(
        "  {}",
        format!("Collecting diagnostics from remote workspace: {workspace_name}").yellow()
    );

    println!("\n{}", "Please provide a brief summary of the issue:".bold());
    let summary: String =
        Input::new().with_prompt(">").interact_text().context("Failed to read summary")?;

    println!("\n{}", "Please provide a detailed description of the issue:".bold());
    let description: String =
        Input::new().with_prompt(">").interact_text().context("Failed to read description")?;

    println!("\n{}", "What command did you run when this issue occurred?".bold());
    let command: String = Input::new()
        .with_prompt(">")
        .default("N/A".to_string())
        .interact_text()
        .context("Failed to read command")?;

    println!("\n{}", "Summary of bug report:".bold());
    println!("  Summary: {summary}");
    println!("  Command: {command}");

    let confirmed = Confirm::new()
        .with_prompt("Create bug report?")
        .default(true)
        .interact()
        .context("Failed to read confirmation")?;

    if !confirmed {
        println!("Bug report cancelled.");
        return Ok(());
    }

    report_via_remote(workspace_name, &summary, &description, &command).await
}

/// Forward the report command to the remote workspace via `coder ssh`.
async fn report_via_remote(
    workspace_name: &str,
    summary: &str,
    description: &str,
    command: &str,
) -> Result<()> {
    println!(
        "\n{}",
        format!("📡 Running report on remote workspace '{workspace_name}'...").yellow()
    );

    let escaped_summary = shell_escape(summary);
    let escaped_description = shell_escape(description);
    let escaped_command = shell_escape(command);

    let remote_cmd = format!(
        "cd /home/workspace/scio && devdock report --summary '{escaped_summary}' --description '{escaped_description}' --command '{escaped_command}'"
    );

    let status = Command::new("coder")
        .args(["ssh", workspace_name, "--", &remote_cmd])
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to execute coder ssh — is coder CLI installed?")?;

    if !status.success() {
        anyhow::bail!(
            "Remote report command failed (exit code {})",
            status.code().map_or("unknown".to_string(), |c| c.to_string())
        );
    }

    Ok(())
}

fn shell_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}
