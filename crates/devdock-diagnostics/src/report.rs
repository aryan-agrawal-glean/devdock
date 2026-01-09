//! Main bug reporter orchestration.

use crate::{archive, config, jira, slack, system, tilt, DiagnosticsError, Result};
use colored::Colorize;
use dialoguer::{Confirm, Input};
use tracing::warn;

/// Bug reporter that collects diagnostics and creates Jira tickets.
pub struct BugReporter {
    summary: Option<String>,
    description: Option<String>,
    command: Option<String>,
    interactive: bool,
}

impl BugReporter {
    /// Create a new bug reporter in interactive mode.
    pub fn new() -> Self {
        Self { summary: None, description: None, command: None, interactive: true }
    }

    /// Create a bug reporter with pre-filled information (non-interactive).
    pub fn with_details(summary: String, description: String, command: String) -> Self {
        Self {
            summary: Some(summary),
            description: Some(description),
            command: Some(command),
            interactive: false,
        }
    }

    /// Set whether to run in interactive mode.
    pub fn interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }

    /// Run the bug reporting workflow.
    pub async fn report(mut self) -> Result<String> {
        println!("{}", "\n🐛 DevDock Bug Report".bold().cyan());
        println!("{}", "=".repeat(50).cyan());

        // Collect user input if in interactive mode
        if self.interactive {
            self.collect_user_input()?;
        }

        let summary = self
            .summary
            .ok_or_else(|| DiagnosticsError::SystemInfo("Summary is required".to_string()))?;
        let description = self
            .description
            .ok_or_else(|| DiagnosticsError::SystemInfo("Description is required".to_string()))?;
        let command = self.command.unwrap_or_else(|| "N/A".to_string());

        // Collect system information
        println!("\n{}", "📊 Collecting system information...".yellow());
        let system_info = system::collect_system_info()?;
        println!("  {} System info collected", "✓".green());

        // Check Tilt status and capture snapshot
        println!("\n{}", "🎯 Checking Tilt status...".yellow());
        let temp_dir =
            std::env::temp_dir().join(format!("devdock_snapshot_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir)?;
        let snapshot_file = tilt::capture_tilt_snapshot(&temp_dir)?;

        if snapshot_file.is_some() {
            println!("  {} Tilt snapshot captured", "✓".green());
        } else {
            println!("  {} Tilt not running, skipping snapshot", "⊘".yellow());
        }

        // Collect configuration files (only if Tilt is running)
        let config_files = if snapshot_file.is_some() {
            println!("\n{}", "📁 Collecting configuration files...".yellow());
            let files = config::get_config_files()?;
            println!("  {} {} configuration file(s) collected", "✓".green(), files.len());
            files
        } else {
            Default::default()
        };

        // Get git branch
        let git_branch = config::get_current_git_branch().unwrap_or_else(|_| "unknown".to_string());

        // Create archive
        println!("\n{}", "📦 Creating report archive...".yellow());
        let archive_path =
            archive::create_report_archive(&system_info, snapshot_file.as_ref(), &config_files)?;
        println!("  {} Archive created: {:?}", "✓".green(), archive_path);

        // Format Jira description (in ADF format)
        let full_description = jira::format_description(
            &description,
            &command,
            &git_branch,
            &system_info,
            &config_files,
        );

        // Create Jira ticket
        println!("\n{}", "📝 Creating Jira ticket...".yellow());
        let ticket_key =
            jira::create_jira_ticket(&summary, full_description, &archive_path).await?;
        println!("  {} Jira ticket created: {}", "✓".green(), ticket_key.bold());

        // Post to Slack
        println!("\n{}", "💬 Posting to Slack...".yellow());
        let reporter_email = jira::get_reporter_email().unwrap_or_else(|_| "unknown".to_string());
        match slack::post_to_slack(&ticket_key, &summary, &reporter_email).await {
            Ok(_) => println!("  {} Slack notification sent", "✓".green()),
            Err(e) => warn!("  {} Slack notification failed: {}", "⚠".yellow(), e),
        }

        // Cleanup temporary files
        let temp_files = vec![temp_dir, archive_path.parent().unwrap().to_path_buf()];
        archive::cleanup_temp_files(&temp_files);

        println!("\n{}", "✅ Bug report successfully created!".bold().green());
        println!("\n{}", "View ticket:".bold());
        println!("  {}", format!("https://askscio.atlassian.net/browse/{}", ticket_key).cyan());

        Ok(ticket_key)
    }

    fn collect_user_input(&mut self) -> Result<()> {
        // Get summary
        if self.summary.is_none() {
            println!("\n{}", "Please provide a brief summary of the issue:".bold());
            let summary: String = Input::new()
                .with_prompt(">")
                .interact_text()
                .map_err(|e| DiagnosticsError::SystemInfo(format!("Input error: {}", e)))?;
            self.summary = Some(summary);
        }

        // Get detailed description
        if self.description.is_none() {
            println!("\n{}", "Please provide a detailed description of the issue:".bold());
            let description: String = Input::new()
                .with_prompt(">")
                .interact_text()
                .map_err(|e| DiagnosticsError::SystemInfo(format!("Input error: {}", e)))?;
            self.description = Some(description);
        }

        // Get command
        if self.command.is_none() {
            println!("\n{}", "What command did you run when this issue occurred?".bold());
            let command: String = Input::new()
                .with_prompt(">")
                .default("N/A".to_string())
                .interact_text()
                .map_err(|e| DiagnosticsError::SystemInfo(format!("Input error: {}", e)))?;
            self.command = Some(command);
        }

        // Confirmation
        println!("\n{}", "Summary of bug report:".bold());
        println!("  Summary: {}", self.summary.as_ref().unwrap());
        println!("  Command: {}", self.command.as_ref().unwrap());

        let confirmed = Confirm::new()
            .with_prompt("Create bug report?")
            .default(true)
            .interact()
            .map_err(|e| DiagnosticsError::SystemInfo(format!("Input error: {}", e)))?;

        if !confirmed {
            return Err(DiagnosticsError::SystemInfo("Bug report cancelled by user".to_string()));
        }

        Ok(())
    }
}

impl Default for BugReporter {
    fn default() -> Self {
        Self::new()
    }
}
