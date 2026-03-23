//! Remote DevDock workflow implementation.

use crate::{logging::OperationLogger, output};
use anyhow::{anyhow, Result};
use colored::Colorize;
use devdock_config::ProfileLoader;
use devdock_remote::{check_gcloud_auth, coder, workspace, CoderWorkspaceConfig, RemoteError};
use devdock_state::{
    instance::{Instance, InstanceStatus, SessionOrigin},
    ports::PortAllocator,
    StateManager,
};
use devdock_sync::daemon as sync_daemon;
use std::path::PathBuf;
use std::time::Instant;
use tracing::{info, warn};

/// Configuration for starting remote DevDock.
pub struct RemoteStartConfig {
    pub profile: String,
    pub workspace_name: Option<String>,
    pub branch: Option<String>,
    pub region: String,
    pub port_base: Option<u16>,
    pub vm: Option<String>,
    pub auto_forward: bool,
    pub services: Option<String>,
    pub verbose: bool,
    pub log_level: String,
}

/// Start remote DevDock workflow.
pub async fn start_remote(config: RemoteStartConfig) -> Result<()> {
    info!("Starting remote DevDock workflow");

    // Preflight check: verify gcloud auth is valid before any remote operations
    // This fails fast with a clear error message, avoiding wasted time on workspace
    // provisioning that would fail later due to expired credentials.
    if let Err(e) = check_gcloud_auth() {
        match e {
            RemoteError::GcloudNotInstalled => {
                output::error("gcloud CLI not found");
                println!();
                println!("Remote DevDock requires the gcloud CLI to sync credentials.");
                println!("Install gcloud: {}", "https://cloud.google.com/sdk/docs/install".cyan());
                return Err(anyhow!("{}", e));
            }
            RemoteError::GcloudAuthFailed => {
                output::error("gcloud auth is missing or expired");
                println!();
                println!("Remote DevDock requires valid gcloud authentication.");
                println!("Run the following command and retry:");
                println!();
                println!("  {}", "gcloud auth login --update-adc".cyan().bold());
                println!();
                return Err(anyhow!("{}", e));
            }
            _ => {
                return Err(anyhow!("{}", e));
            }
        }
    }

    let RemoteStartConfig {
        profile,
        workspace_name,
        branch,
        region,
        port_base,
        vm,
        auto_forward,
        services,
        verbose,
        log_level,
    } = config;

    // Get current git info
    let worktree = get_current_worktree()?;
    let current_branch = get_current_branch()?;

    // Determine branch for workspace
    // If --branch specified: use it and validate
    // If not specified: use "master" (keep current behavior)
    let (branch_for_workspace, use_branch_mode) = if let Some(target_branch) = &branch {
        // Validate the specified branch
        validate_branch_exists(target_branch)?;

        // Warn if different from current branch
        if target_branch != &current_branch {
            output::warn(&format!(
                "Local branch is '{}' but remote will use '{}'",
                current_branch, target_branch
            ));
            println!("  Consider checking out locally: git checkout {}", target_branch);
            println!();
        }

        (target_branch.clone(), true)
    } else {
        // No --branch specified: use master (current behavior)
        ("master".to_string(), false)
    };

    // Determine workspace name and sanitize it (replace slashes and special chars)
    let workspace_name =
        workspace_name.or_else(|| vm.clone()).unwrap_or_else(|| current_branch.clone());
    let workspace_name = sanitize_workspace_name(&workspace_name);

    // Find profile path and load profile for GCP project
    let profile_path = find_profile(&profile)?;
    let loader = ProfileLoader::new();
    let loaded_profile = loader.load_from_path(&profile_path)?;

    // Load devdock.toml for remote configuration
    let devdock_toml_path = std::path::Path::new(&worktree).join(".devdock/devdock.toml");
    let (coder_template, remote_workspace_path) = if devdock_toml_path.exists() {
        match devdock_core::ProjectConfig::load(&devdock_toml_path) {
            Ok(pc) => {
                let template = pc.remote.template.clone().unwrap_or_else(|| "devdock".to_string());
                let ws_path = pc.remote.resolved_workspace_path(&pc.project.name);
                (template, ws_path)
            }
            Err(_) => ("devdock".to_string(), "/home/workspace/scio".to_string()),
        }
    } else {
        ("devdock".to_string(), "/home/workspace/scio".to_string())
    };

    // Register instance in database
    let state = StateManager::new().await?;
    let instance_id = Instance::generate_id(SessionOrigin::Dev);

    // Calculate port offset
    let offset = if let Some(base) = port_base {
        base
    } else {
        let port_allocator = PortAllocator::new(state.clone()).await;
        port_allocator.allocate().await?
    };

    let instance = Instance {
        id: instance_id.clone(),
        workspace_path: worktree.clone(),
        workspace_name: Some(workspace_name.clone()),
        branch: branch_for_workspace.clone(),
        profile: profile.clone(),
        mode: "remote".to_string(),
        status: InstanceStatus::Starting,
        port_base: offset,
        created_at: now(),
        updated_at: now(),
    };

    state.register_instance(&instance).await?;

    // Track total time
    let overall_start = Instant::now();

    // Create operation logger
    let logger = OperationLogger::new(&instance_id)?;
    logger.log_section("Configuration")?;
    logger.log(&format!("Profile: {profile}"))?;
    logger.log(&format!("Local Branch: {}", current_branch))?;
    logger.log(&format!("Target Branch: {}", branch_for_workspace))?;
    logger.log(&format!("Workspace: {workspace_name}"))?;
    logger.log(&format!("Region: {region}"))?;
    logger.log(&format!("Port Base: {offset}"))?;
    logger.log(&format!("Auto-forward: {auto_forward}"))?;
    logger.log(&format!("Verbose: {verbose}"))?;

    // Progress will be shown via spinners for non-verbose mode

    // Display initial info (cleaner, more compact)
    println!();
    if verbose {
        output::header("Starting Remote DevDock");
        println!("  {}: {}", "Profile".cyan(), profile);
        println!("  {}: {}", "Local Branch".cyan(), current_branch);
        println!("  {}: {}", "Target Branch".cyan(), branch_for_workspace);
        println!("  {}: {}", "Workspace".cyan(), workspace_name.green());
        println!("  {}: {}", "Template".cyan(), coder_template);
        println!("  {}: {}", "Region".cyan(), region);
        println!("  {}: {}", "DevDock ID".cyan(), Instance::display_id(&instance_id));
        println!("  {}: {}", "Port Base".cyan(), offset);
        println!();
        println!("{}", "⏱️  Expected time: ~1-2 minutes total".yellow());
        println!("   • Creating workspace: ~30s");
        println!("   • Config + Starting: ~30-45s");
        println!();
        println!("📋 Session logs: {}", logger.path().display().to_string().dimmed());
        println!("   Monitor: tail -f {}", logger.path().display().to_string().dimmed());
        println!();
    } else {
        // Compact header
        println!("{}", "╭───────────────────────────────────────────╮".cyan());
        println!("{}", format!("│  🚀 Remote DevDock: {profile}@{workspace_name}").cyan());
        println!("{}", "╰───────────────────────────────────────────╯".cyan());
        println!();
        println!(
            "{} | {} | {}",
            format!("Profile: {profile}").dimmed(),
            format!("Branch: {}", branch_for_workspace).dimmed(),
            format!("ID: {}", Instance::display_id(&instance_id)).dimmed()
        );
        println!();
        println!("⏱️  Expected: ~1-2 min");
        println!();
    }

    // Execute main setup logic with error handling to update status on failure
    let setup_result = async {
        // Create or start workspace
        let workspace_name = if let Some(existing) = vm {
            logger.log_section("Using Existing Workspace")?;
            logger.log(&format!("Workspace: {existing}"))?;
            output::info(&format!("Using existing workspace: {existing}"));
            coder::start_workspace(&existing)?;
            logger.log("Workspace started successfully")?;
            existing
        } else {
            // Check if workspace already exists
            logger.log(&format!("Checking if workspace '{workspace_name}' exists..."))?;
            let exists = coder::workspace_exists(&workspace_name)?;
            logger.log(&format!("Workspace exists: {exists}"))?;

            if exists {
                if verbose {
                    println!();
                    output::warn(&format!("Workspace '{workspace_name}' already exists!"));
                    println!();
                } else {
                    println!(
                        "{}",
                        "⚠️  Existing workspace - restarting (use --recreate to start fresh)"
                            .to_string()
                            .yellow()
                    );
                    println!();
                }

                if verbose {
                    println!("  What would you like to do?");
                    println!("  1) Continue with existing workspace (clean and restart DevDock)");
                    println!("  2) Delete workspace and create fresh");
                    println!("  3) Cancel");
                    println!();
                } else {
                    println!("  1) Continue with existing (clean and restart)");
                    println!("  2) Delete and recreate");
                    println!("  3) Cancel");
                    println!();
                }
                print!("  Enter choice (1-3): ");
                use std::io::{self, Write};
                io::stdout().flush()?;

                let mut choice = String::new();
                io::stdin().read_line(&mut choice)?;
                let choice = choice.trim();

                match choice {
                    "1" => {
                        logger.log_section("Using Existing Workspace")?;
                        logger.log(&format!("Workspace: {workspace_name}"))?;
                        logger.log("User chose to continue with existing workspace")?;

                        if !verbose {
                            println!();
                        }

                        // Start workspace if stopped (silent in non-verbose mode)
                        if verbose {
                            output::info(&format!("Using existing workspace: {workspace_name}"));
                        }
                        coder::start_workspace(&workspace_name)?;
                        logger.log("Workspace started")?;

                        // Cleanup existing DevDock session
                        if verbose {
                            output::info("Cleaning up existing DevDock session...");
                        }
                        workspace::cleanup_existing_devdock(
                            &workspace_name,
                            Some(logger.path()),
                            &remote_workspace_path,
                        )?;
                        logger.log("Existing DevDock session cleaned up")?;

                        // Note: Gcloud credentials will be re-synced in workspace::start_remote_devdock
                        // No need to sync them here

                        workspace_name.clone()
                    }
                    "2" => {
                        logger.log_section("Deleting and Recreating Workspace")?;
                        logger.log(&format!("Workspace: {workspace_name}"))?;
                        logger.log("User chose to delete and recreate workspace")?;

                        output::info(&format!("Deleting existing workspace: {workspace_name}"));
                        coder::delete_workspace(&workspace_name)?;
                        logger.log("Workspace deleted")?;

                        // Wait a moment for deletion to complete
                        std::thread::sleep(std::time::Duration::from_secs(2));

                        // Create new workspace
                        logger.log_section("Creating Workspace")?;
                        logger.log(&format!("Template: {coder_template}"))?;
                        logger.log(&format!("Workspace name: {workspace_name}"))?;
                        logger.log(&format!("Region: {region}"))?;

                        let spinner =
                            output::create_spinner("Creating Coder workspace (1-2 minutes)...");
                        let config = CoderWorkspaceConfig {
                            template: coder_template.clone(),
                            workspace_name: workspace_name.clone(),
                            region: region.clone(),
                            port_base: Some(offset),
                            profile: profile.clone(),
                            branch: branch_for_workspace.clone(),
                        };

                        let ws_name = workspace::provision_workspace(&config).await?;
                        spinner.finish_and_clear();
                        output::success("Workspace created");
                        logger.log("Workspace created successfully")?;

                        logger.log(&format!("Workspace SSH ready: coder ssh {ws_name}"))?;
                        ws_name
                    }
                    "3" | "" => {
                        output::info("Cancelled");
                        return Err(anyhow!("User cancelled workspace setup"));
                    }
                    _ => {
                        output::error("Invalid choice. Cancelled.");
                        return Err(anyhow!("Invalid choice - setup cancelled"));
                    }
                }
            } else {
                // Workspace doesn't exist, create it
                logger.log_section("Creating Workspace")?;
                logger.log(&format!("Template: {coder_template}"))?;
                logger.log(&format!("Workspace name: {workspace_name}"))?;
                logger.log(&format!("Region: {region}"))?;

                // Create workspace with spinner
                let spinner = output::create_spinner("Creating Coder workspace (1-2 minutes)...");
                let config = CoderWorkspaceConfig {
                    template: coder_template.clone(),
                    workspace_name: workspace_name.clone(),
                    region: region.clone(),
                    port_base: Some(offset),
                    profile: profile.clone(),
                    branch: branch_for_workspace.clone(),
                };

                let ws_name = workspace::provision_workspace(&config).await?;
                spinner.finish_and_clear();
                output::success("Workspace created");
                logger.log("Workspace created successfully")?;

                logger.log(&format!("Workspace SSH ready: coder ssh {ws_name}"))?;
                ws_name
            }
        };

        // Start DevDock on remote with progress reporting
        println!();
        logger.log_section("Syncing Configuration")?;
        logger.log("Syncing profile, custom.ini, and gcloud credentials...")?;

        logger.log_section("Starting DevDock")?;
        logger.log(&format!("Profile: {profile}"))?;
        logger.log(&format!("Services filter: {services:?}"))?;
        logger.log(&format!(
            "Command: devdock start {} {}",
            profile,
            services.as_ref().map(|s| format!("-s {s}")).unwrap_or_default()
        ))?;
        logger.log(&format!(
            "Remote logs: coder ssh {} -- tail -f /tmp/devdock-start.log",
            &workspace_name
        ))?;

        // Step 1: Config sync
        let config_start = Instant::now();

        let spinner1 = if verbose {
            println!(); // Newline for substeps
            output::info("Syncing configuration files to remote...");
            None
        } else {
            Some(output::create_spinner("Config"))
        };

        // Step 2: DevDock start (spinner created by callback after config completes)
        let spinner2 = std::sync::Arc::new(std::sync::Mutex::new(None::<indicatif::ProgressBar>));
        let spinner2_clone = spinner2.clone();
        let devdock_start_time = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
        let devdock_start_clone = devdock_start_time.clone();

        let config_duration_ref = std::sync::Arc::new(std::sync::Mutex::new(config_start));
        let config_duration_clone = config_duration_ref.clone();

        let on_config_complete = move || {
            if !verbose {
                let config_duration = config_duration_clone.lock().unwrap().elapsed().as_secs();
                if let Some(s) = spinner1 {
                    s.finish_with_message(format!(
                        "Config {}",
                        format!("✓ {config_duration}s").green()
                    ));
                }

                // Start spinner for DevDock startup phase
                let s2 = output::create_spinner("Starting");
                *spinner2_clone.lock().unwrap() = Some(s2);
                *devdock_start_clone.lock().unwrap() = Instant::now();
            }
        };

        // The actual config sync and devdock start happens inside workspace::start_remote_devdock
        let port_mgr = workspace::start_remote_devdock(
            &workspace_name,
            &profile,
            &profile_path,
            &instance_id,
            services.as_deref(),
            auto_forward,
            offset,
            &remote_workspace_path,
            Some(logger.path()),
            verbose,
            Some(on_config_complete),
        )
        .await?;

        // Finish DevDock start spinner
        if verbose {
            output::success("DevDock started successfully");
        } else {
            let devdock_duration = devdock_start_time.lock().unwrap().elapsed().as_secs();
            if let Some(s) = spinner2.lock().unwrap().take() {
                s.finish_with_message(format!(
                    "Starting {}",
                    format!("✓ {devdock_duration}s").green()
                ));
            }
        }
        logger.log("DevDock started successfully")?;

        // Step 2: Save port mappings (if auto-forwarding)
        if let Some(mgr) = &port_mgr {
            let step2_start = Instant::now();

            let spinner2 = if verbose { None } else { Some(output::create_spinner("Ports")) };

            let mappings: Vec<_> = mgr
                .mappings
                .iter()
                .map(|m| {
                    (
                        m.service_name.clone(),
                        format!("{:?}", m.port_type),
                        m.local_port,
                        m.remote_port,
                        m.process_pid,
                    )
                })
                .collect();

            state.save_port_mappings(&instance_id, &mappings).await?;

            let step2_duration = step2_start.elapsed().as_secs();
            if let Some(s) = spinner2 {
                s.finish_with_message(format!("Ports {}", format!("✓ {step2_duration}s").green()));
            }
        }

        // Clean up stale sync files from old instances first
        cleanup_stale_sync_files(&state).await?;

        // Step 3 (or 2 if no ports): Auto-start file sync (always enabled)
        let step_sync_start = Instant::now();

        let spinner_sync = if verbose {
            println!();
            Some(output::create_spinner("Starting file sync..."))
        } else {
            Some(output::create_spinner("Sync"))
        };

        let repo_path = PathBuf::from(&worktree);
        let remote_path = std::path::PathBuf::from(&remote_workspace_path);
        let sync_result = match sync_daemon::start_daemon(
            &instance_id,
            &workspace_name,
            &repo_path,
            &remote_path,
            false,           // full_mode = false (don't sync everything)
            use_branch_mode, // branch_mode = true when --branch specified
            &log_level,
        ) {
            Ok(_) => {
                state
                    .save_sync_session(&instance_id, &workspace_name, None, false, "running")
                    .await?;

                let step_sync_duration = step_sync_start.elapsed().as_secs();

                if let Some(s) = spinner_sync {
                    if verbose {
                        s.finish_with_message("File sync started ✓".green().to_string());
                    } else {
                        s.finish_with_message(format!(
                            "Sync {}",
                            format!("✓ {step_sync_duration}s").green()
                        ));
                    }
                }
                Ok(())
            }
            Err(e) => {
                warn!("Failed to start file sync: {}", e);
                if let Some(s) = spinner_sync {
                    if verbose {
                        s.finish_with_message("File sync failed ⚠️".yellow().to_string());
                    } else {
                        s.finish_with_message(format!("Sync {}", "⚠️ failed".yellow()));
                    }
                }

                if verbose {
                    output::warn("Start manually: devdock sync start");
                }
                Err(e)
            }
        };

        // Continue even if sync fails
        let _ = sync_result;

        // Update instance status
        state.update_status(&instance_id, InstanceStatus::Running).await?;

        // Calculate total time
        let total_seconds = overall_start.elapsed().as_secs();
        let total_time = output::format_elapsed(total_seconds);

        // Log completion
        logger.log_section("Setup Complete")?;
        logger.log(&format!("Total time: {total_time}"))?;
        logger.log(&format!("Workspace: {workspace_name}"))?;
        logger.log(&format!("Tilt UI: http://localhost:{}", offset + 10350))?;

        // Print success message (clean and actionable)
        println!();

        let gcp_logs_url = format!(
            "https://console.cloud.google.com/logs/query;query=labels.devdock_id%3D%22{}%22;summaryFields=labels%252Flogger_source:false:32:beginning?project={}",
            instance_id,
            loaded_profile.project
        );

        if verbose {
            output::header("🚀 Remote DevDock is Ready!");
            println!("{}", format!("Total time: {total_time}").green().bold());
            println!();
            println!("Access your remote DevDock:");
            println!("  {}", "──────────────────────────────────────────────────────────".cyan());
            println!("  {}: {}", "Workspace".cyan(), workspace_name);
            println!("  {}: coder ssh {}", "SSH".cyan(), workspace_name);
            println!(
                "  {}: {}",
                "Tilt UI".cyan(),
                format!("http://localhost:{}", offset + 10350).cyan()
            );
            println!("  {}: {}", "Session".cyan(), instance_id.dimmed());
            println!("  {}: {}", "Logs".cyan(), gcp_logs_url.underline());
            println!("  {}", "──────────────────────────────────────────────────────────".cyan());
            println!();

            if auto_forward {
                println!("  {}: {}", "Port forwarding".cyan(), "enabled".green());
                println!("  View port mappings: {}", "devdock ports list".cyan());
            } else {
                println!(
                    "  💡 Tip: Forward ports with: {}",
                    "devdock ports forward --all".yellow()
                );
            }

            println!();
            println!("Additional commands:");
            println!("  {}: devdock status", "Check health".dimmed());
            println!("  {}: devdock sync status", "Sync status".dimmed());
            println!("  {}: devdock stop", "Stop session".dimmed());
            println!();
        } else {
            // Compact success output
            println!("{}", "╭────────────────────────────────────────╮".green());
            println!("{}", format!("│  ✅ Ready in {total_time}                     │").green());
            println!("{}", "╰────────────────────────────────────────╯".green());
            println!();
            println!("  {}", "──────────────────────────────────────────────────────────".cyan());
            println!(
                "  {} {}",
                "🌐 Tilt UI:".cyan(),
                format!("http://localhost:{}", offset + 10350).cyan().underline()
            );
            println!("  {} coder ssh {}", "📁 SSH:    ".dimmed(), workspace_name.dimmed());
            println!("  {} {}", "🔑 Session:".cyan(), instance_id.dimmed());
            println!("  {} {}", "📋 Logs:   ".cyan(), gcp_logs_url.underline());
            println!("  {}", "──────────────────────────────────────────────────────────".cyan());
            println!();

            // Show actionable next steps based on context
            println!("{}", "💡 Next steps:".bold());

            if auto_forward {
                println!(
                    "   {} {}  {}",
                    "•".cyan(),
                    "devdock ports list".cyan(),
                    "(view port mappings)".dimmed()
                );
            } else {
                println!(
                    "   {} {}  {}",
                    "•".cyan(),
                    "devdock ports forward --all".cyan(),
                    "(forward service ports)".dimmed()
                );
            }

            println!(
                "   {} {}  {}",
                "•".cyan(),
                "devdock status".cyan(),
                "(check service health)".dimmed()
            );
            println!(
                "   {} {}  {}",
                "•".cyan(),
                format!("open http://localhost:{}", offset + 10350).cyan(),
                "(view dashboard)".dimmed()
            );
            println!();
        }

        // Send desktop notification
        output::notify(
            "DevDock Remote Ready! 🚀",
            &format!(
                "Workspace '{}' ready in {}. Tilt UI at localhost:{}",
                workspace_name,
                total_time,
                offset + 10350
            ),
        );

        Ok::<(), anyhow::Error>(())
    }
    .await;

    // Handle result and update status accordingly
    match setup_result {
        Ok(()) => {
            // Success - status already updated to Running above
            Ok(())
        }
        Err(e) => {
            // Failed - update status to Stopped
            info!("Setup failed, updating instance status to Stopped");
            if let Err(status_err) =
                state.update_status(&instance_id, InstanceStatus::Stopped).await
            {
                warn!("Failed to update instance status: {}", status_err);
            }

            logger.log_section("Setup Failed")?;
            logger.log(&format!("Error: {e}"))?;
            logger.log(&format!(
                "Instance {} marked as Stopped",
                Instance::display_id(&instance_id)
            ))?;

            output::error(&format!("DevDock setup failed: {e}"));
            println!();
            println!("To clean up: devdock stop -i {}", Instance::display_id(&instance_id));
            Err(e)
        }
    }
}

/// Get current git worktree path.
fn get_current_worktree() -> Result<String> {
    use std::process::Command;

    let output = Command::new("git").args(["rev-parse", "--show-toplevel"]).output()?;

    if !output.status.success() {
        return Err(anyhow!("Not in a git repository"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Get current git branch name.
fn get_current_branch() -> Result<String> {
    use std::process::Command;

    let output = Command::new("git").args(["rev-parse", "--abbrev-ref", "HEAD"]).output()?;

    if !output.status.success() {
        return Err(anyhow!("Failed to get branch name"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Validate that a branch exists on remote, auto-pushing if only exists locally.
fn validate_branch_exists(branch: &str) -> Result<()> {
    use std::process::Command;

    // Check remote first
    let remote_check = Command::new("git")
        .args(["ls-remote", "origin", &format!("refs/heads/{}", branch)])
        .output()?;

    if remote_check.status.success() && !String::from_utf8_lossy(&remote_check.stdout).is_empty() {
        info!("Branch '{}' exists on remote", branch);
        return Ok(());
    }

    // Check if exists locally
    let local_check = Command::new("git").args(["rev-parse", "--verify", branch]).output()?;

    if !local_check.status.success() {
        return Err(anyhow!(
            "Branch '{}' not found locally or on remote. Create it first or check for typos.",
            branch
        ));
    }

    // Branch exists locally but not remotely - auto-push
    output::info(&format!("Branch '{}' exists locally but not on remote", branch));
    output::info("Pushing branch to remote...");

    let push_output = Command::new("git").args(["push", "-u", "origin", branch]).output()?;

    if !push_output.status.success() {
        let stderr = String::from_utf8_lossy(&push_output.stderr);
        return Err(anyhow!("Failed to push branch '{}' to remote: {}", branch, stderr));
    }

    output::success(&format!("Branch '{}' pushed to remote successfully", branch));
    Ok(())
}

/// Sanitize workspace name for use with Coder.
/// Replaces non-alphanumeric characters with hyphens and truncates to 32 chars.
/// Coder requires names to be alphanumeric with hyphens, but cannot start or end with hyphens.
fn sanitize_workspace_name(name: &str) -> String {
    // Replace non-alphanumeric (except hyphens) with hyphens
    let sanitized: String =
        name.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' }).collect();

    // Truncate to 32 characters (Coder workspace name limit)
    let truncated: String = sanitized.chars().take(32).collect();

    // Trim leading/trailing hyphens (Coder doesn't allow them)
    truncated.trim_matches('-').to_string()
}

/// Find profile file path.
fn find_profile(profile: &str) -> Result<PathBuf> {
    let devdock_root = std::env::var("DEVDOCK_ROOT")
        .or_else(|_| std::env::var("PWD"))
        .unwrap_or_else(|_| ".".to_string());

    let base = PathBuf::from(devdock_root).join("devdock/profiles");

    // Try user_profiles first
    let user_profile = base.join("user_profiles").join(format!("{profile}.yaml"));
    if user_profile.exists() {
        return Ok(user_profile);
    }

    // Try integration_tests
    let test_profile = base.join("integration_tests").join(format!("{profile}.yaml"));
    if test_profile.exists() {
        return Ok(test_profile);
    }

    // Try base profiles
    let base_profile = base.join(format!("{profile}.yaml"));
    if base_profile.exists() {
        return Ok(base_profile);
    }

    Err(anyhow!("Profile not found: {profile}"))
}

/// Get current timestamp.
fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

/// Clean up sync files for instances that no longer exist in the database.
async fn cleanup_stale_sync_files(state_manager: &StateManager) -> Result<()> {
    use std::collections::HashSet;
    use std::fs;

    // Get all active instance IDs
    let instances = state_manager.list_instances().await?;
    let active_ids: HashSet<String> = instances.iter().map(|i| i.id.clone()).collect();

    // Get sync directory
    let sync_dir = dirs::home_dir()
        .ok_or_else(|| anyhow!("Cannot determine home directory"))?
        .join(".devdock/sync");

    if !sync_dir.exists() {
        return Ok(());
    }

    let entries = fs::read_dir(&sync_dir)?;
    let mut cleaned_count = 0;

    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().to_string();

        // Extract devdock_id from filename
        if let Some(devdock_id) = extract_devdock_id_from_filename(&file_name) {
            if !active_ids.contains(&devdock_id) {
                // This is a stale file from a stopped/removed instance
                if fs::remove_file(entry.path()).is_ok() {
                    cleaned_count += 1;
                }
            }
        }
    }

    if cleaned_count > 0 {
        info!("Cleaned up {} stale sync file(s)", cleaned_count);
    }

    Ok(())
}

/// Extract devdock ID from sync filename.
/// Handles both legacy hex-timestamp IDs and new prefixed UUIDs (dev-{uuid}, bot-{uuid}).
fn extract_devdock_id_from_filename(filename: &str) -> Option<String> {
    let base = filename
        .trim_end_matches(".stats.json")
        .trim_end_matches(".history.jsonl")
        .trim_end_matches(".errors.jsonl")
        .trim_end_matches(".queue.json")
        .trim_end_matches(".pid")
        .trim_end_matches(".excludes")
        .trim_end_matches(".hb")
        .trim_end_matches(".stats")
        .trim_end_matches(".log")
        .trim_end_matches("_portforward");

    if base.is_empty() || base == filename {
        return None;
    }

    Some(base.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_id_from_prefixed_uuid_pid() {
        let id = extract_devdock_id_from_filename("dev-550e8400-e29b-41d4-a716-446655440000.pid");
        assert_eq!(id.as_deref(), Some("dev-550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn extract_id_from_bot_prefixed_uuid_pid() {
        let id = extract_devdock_id_from_filename("bot-550e8400-e29b-41d4-a716-446655440000.pid");
        assert_eq!(id.as_deref(), Some("bot-550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn extract_id_from_legacy_hex_pid() {
        let id = extract_devdock_id_from_filename("1945a3b2c1f.pid");
        assert_eq!(id.as_deref(), Some("1945a3b2c1f"));
    }

    #[test]
    fn extract_id_from_legacy_uuid_pid() {
        let id = extract_devdock_id_from_filename("550e8400-e29b-41d4-a716-446655440000.pid");
        assert_eq!(id.as_deref(), Some("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn extract_id_from_various_extensions() {
        let base = "dev-550e8400-e29b-41d4-a716-446655440000";
        for ext in [".pid", ".excludes", ".hb", ".stats", ".log"] {
            let filename = format!("{base}{ext}");
            assert_eq!(
                extract_devdock_id_from_filename(&filename).as_deref(),
                Some(base),
                "failed for extension {ext}"
            );
        }
    }

    #[test]
    fn extract_id_from_portforward_pid() {
        let id = extract_devdock_id_from_filename(
            "dev-550e8400-e29b-41d4-a716-446655440000_portforward.pid",
        );
        assert_eq!(id.as_deref(), Some("dev-550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn extract_id_from_stats_json() {
        let id =
            extract_devdock_id_from_filename("dev-550e8400-e29b-41d4-a716-446655440000.stats.json");
        assert_eq!(id.as_deref(), Some("dev-550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn extract_id_from_history_jsonl() {
        let id = extract_devdock_id_from_filename(
            "dev-550e8400-e29b-41d4-a716-446655440000.history.jsonl",
        );
        assert_eq!(id.as_deref(), Some("dev-550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn extract_id_returns_none_for_unrecognized() {
        assert_eq!(extract_devdock_id_from_filename("readme.txt"), None);
        assert_eq!(extract_devdock_id_from_filename(".gitkeep"), None);
        assert_eq!(extract_devdock_id_from_filename(""), None);
    }
}
