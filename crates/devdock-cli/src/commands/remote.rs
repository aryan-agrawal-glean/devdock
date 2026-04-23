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

/// Stage names for `failed_at_stage` telemetry field.
mod stage {
    pub const WORKSPACE_PROVISION: &str = "workspace_provision";
    pub const CONFIG_SYNC: &str = "config_sync";
    pub const REMOTE_DEVDOCK_START: &str = "remote_devdock_start";
    pub const PORT_FORWARD: &str = "port_forward";
    pub const SYNC_DAEMON: &str = "sync_daemon";
}

/// Configuration for starting remote DevDock.
pub struct RemoteStartConfig {
    pub profile: String,
    pub workspace_name: Option<String>,
    pub branch: Option<String>,
    pub region: String,
    pub port_base: Option<u16>,
    pub disk_size: Option<u32>,
    pub vm: Option<String>,
    pub auto_forward: bool,
    pub services: Option<String>,
    pub verbose: bool,
    pub log_level: String,
}

/// Start remote DevDock workflow.
pub async fn start_remote(config: RemoteStartConfig) -> Result<()> {
    info!("Starting remote DevDock workflow");

    let command_start_time = chrono::Utc::now();
    let wall_start = std::time::Instant::now();

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
    let auth_check_secs = wall_start.elapsed().as_secs_f64();

    let RemoteStartConfig {
        profile,
        workspace_name,
        branch,
        region,
        port_base,
        disk_size,
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
        println!("  {}: devdock-glean", "Template".cyan());
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

    // Telemetry phase tracking (shared between outer scope and setup_result async block)
    let t_workspace_ready: std::sync::Arc<std::sync::Mutex<Option<f64>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let t_config_sync: std::sync::Arc<std::sync::Mutex<Option<f64>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let t_tilt_up: std::sync::Arc<std::sync::Mutex<Option<f64>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let t_port_forward: std::sync::Arc<std::sync::Mutex<Option<f64>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let t_sync_daemon: std::sync::Arc<std::sync::Mutex<Option<f64>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let t_workspace_existed: std::sync::Arc<std::sync::Mutex<bool>> =
        std::sync::Arc::new(std::sync::Mutex::new(false));
    // Tracks which stage is currently executing; on error, this identifies failed_at_stage.
    let t_current_stage: std::sync::Arc<std::sync::Mutex<&'static str>> =
        std::sync::Arc::new(std::sync::Mutex::new(stage::WORKSPACE_PROVISION));

    // Execute main setup logic with error handling to update status on failure
    let setup_result = async {
        // Create or start workspace
        let (workspace_name, workspace_existed_flag) = if let Some(existing) = vm {
            logger.log_section("Using Existing Workspace")?;
            logger.log(&format!("Workspace: {existing}"))?;
            output::info(&format!("Using existing workspace: {existing}"));
            coder::start_workspace(&existing)?;
            logger.log("Workspace started successfully")?;
            (existing, true)
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
                        workspace::cleanup_existing_devdock(&workspace_name, Some(logger.path()))?;
                        logger.log("Existing DevDock session cleaned up")?;

                        // Note: Gcloud credentials will be re-synced in workspace::start_remote_devdock
                        // No need to sync them here

                        (workspace_name.clone(), true)
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
                        logger.log("Template: devdock-glean")?;
                        logger.log(&format!("Workspace name: {workspace_name}"))?;
                        logger.log(&format!("Region: {region}"))?;

                        let spinner =
                            output::create_spinner("Creating Coder workspace (1-2 minutes)...");
                        let config = CoderWorkspaceConfig {
                            template: "devdock-glean".to_string(),
                            workspace_name: workspace_name.clone(),
                            region: region.clone(),
                            port_base: Some(offset),
                            profile: profile.clone(),
                            branch: branch_for_workspace.clone(),
                            disk_size,
                        };

                        let ws_name = workspace::provision_workspace(&config).await?;
                        spinner.finish_and_clear();
                        output::success("Workspace created");
                        logger.log("Workspace created successfully")?;

                        logger.log(&format!("Workspace SSH ready: coder ssh {ws_name}"))?;
                        (ws_name, false)
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
                logger.log("Template: devdock-glean")?;
                logger.log(&format!("Workspace name: {workspace_name}"))?;
                logger.log(&format!("Region: {region}"))?;

                // Create workspace with spinner
                let spinner = output::create_spinner("Creating Coder workspace (1-2 minutes)...");
                let config = CoderWorkspaceConfig {
                    template: "devdock-glean".to_string(),
                    workspace_name: workspace_name.clone(),
                    region: region.clone(),
                    port_base: Some(offset),
                    profile: profile.clone(),
                    branch: branch_for_workspace.clone(),
                    disk_size,
                };

                let ws_name = workspace::provision_workspace(&config).await?;
                spinner.finish_and_clear();
                output::success("Workspace created");
                logger.log("Workspace created successfully")?;

                logger.log(&format!("Workspace SSH ready: coder ssh {ws_name}"))?;
                (ws_name, false)
            }
        };
        let workspace_ready_secs = wall_start.elapsed().as_secs_f64();
        *t_workspace_ready.lock().unwrap() = Some(workspace_ready_secs);
        *t_workspace_existed.lock().unwrap() = workspace_existed_flag;
        *t_current_stage.lock().unwrap() = stage::CONFIG_SYNC;

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
        let t_config_sync_clone = t_config_sync.clone();
        let t_current_stage_clone = t_current_stage.clone();
        let wall_start_clone = wall_start;

        let on_config_complete = move || {
            // Record config_sync_secs and advance stage to REMOTE_DEVDOCK_START here,
            // so that any failure inside start_remote_devdock *before* this callback
            // fires is correctly attributed to CONFIG_SYNC, not REMOTE_DEVDOCK_START.
            *t_config_sync_clone.lock().unwrap() =
                Some(wall_start_clone.elapsed().as_secs_f64());
            *t_current_stage_clone.lock().unwrap() = stage::REMOTE_DEVDOCK_START;

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
            Some(logger.path()),
            verbose,
            Some(on_config_complete),
        )
        .await?;

        *t_tilt_up.lock().unwrap() = Some(wall_start.elapsed().as_secs_f64());
        *t_current_stage.lock().unwrap() = stage::PORT_FORWARD;

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
            *t_port_forward.lock().unwrap() = Some(wall_start.elapsed().as_secs_f64());

            let step2_duration = step2_start.elapsed().as_secs();
            if let Some(s) = spinner2 {
                s.finish_with_message(format!("Ports {}", format!("✓ {step2_duration}s").green()));
            }
        }
        *t_current_stage.lock().unwrap() = stage::SYNC_DAEMON;

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
        let sync_result = match sync_daemon::start_daemon(
            &instance_id,
            &workspace_name,
            &repo_path,
            false,           // full_mode = false (don't sync everything)
            use_branch_mode, // branch_mode = true when --branch specified
            &log_level,
            &loaded_profile.sync.force_patterns,
        ) {
            Ok(_) => {
                state
                    .save_sync_session(&instance_id, &workspace_name, None, false, "running")
                    .await?;

                *t_sync_daemon.lock().unwrap() = Some(wall_start.elapsed().as_secs_f64());
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

        let start_time = (chrono::Utc::now() - chrono::Duration::minutes(5))
            .format("%Y-%m-%dT%H:%M:%SZ");
        let gcp_logs_url = format!(
            "https://console.cloud.google.com/logs/query;query=labels.devdock_id%3D%22{}%22;summaryFields=labels%252Flogger_source:false:32:beginning;startTime={};duration=PT6H?project={}",
            instance_id,
            start_time,
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
                println!(
                    "         Use {} to skip port offset",
                    "devdock ports forward --all --no-offset".yellow()
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
                    "(forward service ports, add --no-offset to skip offset)".dimmed()
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

    let total_setup_secs = wall_start.elapsed().as_secs_f64();

    // Handle result and update status accordingly
    match setup_result {
        Ok(()) => {
            // Emit success telemetry
            let metrics = build_remote_metrics(
                &instance_id,
                &profile,
                &workspace_name,
                &region,
                &loaded_profile.project,
                Some(&branch_for_workspace),
                command_start_time,
                auth_check_secs,
                &t_workspace_ready,
                &t_config_sync,
                &t_tilt_up,
                &t_port_forward,
                &t_sync_daemon,
                &t_workspace_existed,
                total_setup_secs,
                "success",
                None,
                None,
            );
            emit_remote_telemetry(metrics).await;
            Ok(())
        }
        Err(e) => {
            let failed_stage = *t_current_stage.lock().unwrap();

            // Emit failure telemetry
            let metrics = build_remote_metrics(
                &instance_id,
                &profile,
                &workspace_name,
                &region,
                &loaded_profile.project,
                Some(&branch_for_workspace),
                command_start_time,
                auth_check_secs,
                &t_workspace_ready,
                &t_config_sync,
                &t_tilt_up,
                &t_port_forward,
                &t_sync_daemon,
                &t_workspace_existed,
                total_setup_secs,
                "failed",
                Some(failed_stage),
                Some(&e.to_string()),
            );
            emit_remote_telemetry(metrics).await;

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

/// Build a `RemoteSessionMetrics` from all the tracked timing fields.
#[allow(clippy::too_many_arguments)]
fn build_remote_metrics(
    devdock_id: &str,
    profile: &str,
    workspace_name: &str,
    region: &str,
    project: &str,
    branch: Option<&str>,
    command_start_time: chrono::DateTime<chrono::Utc>,
    auth_check_secs: f64,
    t_workspace_ready: &std::sync::Arc<std::sync::Mutex<Option<f64>>>,
    t_config_sync: &std::sync::Arc<std::sync::Mutex<Option<f64>>>,
    t_tilt_up: &std::sync::Arc<std::sync::Mutex<Option<f64>>>,
    t_port_forward: &std::sync::Arc<std::sync::Mutex<Option<f64>>>,
    t_sync_daemon: &std::sync::Arc<std::sync::Mutex<Option<f64>>>,
    t_workspace_existed: &std::sync::Arc<std::sync::Mutex<bool>>,
    total_setup_secs: f64,
    startup_status: &str,
    failed_at_stage: Option<&str>,
    error_message: Option<&str>,
) -> devdock_telemetry::RemoteSessionMetrics {
    let machine_id = get_machine_id();
    devdock_telemetry::RemoteSessionMetrics {
        devdock_id: devdock_id.to_string(),
        machine_id,
        architecture: std::env::consts::ARCH.to_string(),
        platform: std::env::consts::OS.to_string(),
        project: project.to_string(),
        profile: profile.to_string(),
        branch: branch.map(|b| b.to_string()),
        workspace_name: workspace_name.to_string(),
        region: region.to_string(),
        workspace_existed: *t_workspace_existed.lock().unwrap(),
        binary_version: env!("CARGO_PKG_VERSION").to_string(),
        command_start_time,
        auth_check_secs: Some(auth_check_secs),
        workspace_ready_secs: *t_workspace_ready.lock().unwrap(),
        config_sync_secs: *t_config_sync.lock().unwrap(),
        tilt_up_secs: *t_tilt_up.lock().unwrap(),
        port_forward_secs: *t_port_forward.lock().unwrap(),
        sync_daemon_secs: *t_sync_daemon.lock().unwrap(),
        total_setup_secs: Some(total_setup_secs),
        startup_status: startup_status.to_string(),
        failed_at_stage: failed_at_stage.map(|s| s.to_string()),
        error_message: error_message.map(|s| s.to_string()),
        timestamp: chrono::Utc::now(),
    }
}

/// Fire-and-forget telemetry emission for remote startup metrics.
async fn emit_remote_telemetry(metrics: devdock_telemetry::RemoteSessionMetrics) {
    use devdock_telemetry::{BigQueryConfig, BigQueryEmitter, TelemetryEmitter, TelemetrySnapshot};

    let snapshot = TelemetrySnapshot { remote_session: Some(metrics), ..Default::default() };

    let config = BigQueryConfig {
        project_id: "dev-sandbox-334901".to_string(),
        dataset: "devdock".to_string(),
        use_adc: true,
        ..Default::default()
    };

    match BigQueryEmitter::new(config) {
        Ok(emitter) => match emitter.emit(&snapshot).await {
            Ok(result) => {
                if result.is_success() {
                    tracing::info!("Remote telemetry emitted to BigQuery");
                } else {
                    tracing::warn!(
                        "Partial remote telemetry emission: {} errors",
                        result.failure_count
                    );
                }
            }
            Err(e) => {
                tracing::warn!("Failed to emit remote telemetry to BigQuery: {}", e);
            }
        },
        Err(e) => {
            tracing::debug!(
                "BigQuery telemetry not available: {} (this is normal for local dev)",
                e
            );
        }
    }
}

/// Get a stable machine identifier.
fn get_machine_id() -> String {
    use std::ffi::CStr;

    #[cfg(unix)]
    {
        let mut name = [0 as libc::c_char; 256];
        unsafe {
            if libc::gethostname(name.as_mut_ptr(), 256) == 0 {
                if let Ok(hostname) = CStr::from_ptr(name.as_ptr()).to_str() {
                    return hostname.to_string();
                }
            }
        }
    }

    std::env::var("USER").unwrap_or_else(|_| "unknown".to_string())
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
