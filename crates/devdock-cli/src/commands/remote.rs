//! Remote DevDock workflow implementation.

use crate::{logging::OperationLogger, output};
use anyhow::{anyhow, Result};
use colored::Colorize;
use devdock_config::ProfileLoader;
use devdock_core::profile::Profile;
use devdock_monitor::TiltClient;
use devdock_remote::{
    check_coder_installed, check_gcloud_preflight, coder, inspect_remote_runtime, workspace,
    CoderWorkspaceConfig, PortForwardManager, RemoteError,
};
use devdock_state::{
    instance::{Instance, InstanceStatus, SessionOrigin},
    RemoteSessionRequest, StateManager,
};
use devdock_sync::daemon as sync_daemon;
use std::path::PathBuf;
use std::time::Instant;
use tracing::{info, warn};

/// Stage names for `failed_at_stage` telemetry field.
mod stage {
    pub const OPERATION_LOG: &str = "operation_log";
    pub const WORKSPACE_PROVISION: &str = "workspace_provision";
    pub const WORKSPACE_IDENTITY: &str = "workspace_identity";
    pub const WORKSPACE_CHECKOUT: &str = "workspace_checkout";
    pub const CONFIG_SYNC: &str = "config_sync";
    pub const REMOTE_DEVDOCK_START: &str = "remote_devdock_start";
    pub const PORT_FORWARD: &str = "port_forward";
    pub const SYNC_DAEMON: &str = "sync_daemon";
    pub const TILT_READINESS: &str = "tilt_readiness";
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
    pub copy_git_creds: bool,
    pub services: Option<String>,
    pub verbose: bool,
    pub log_level: String,
}

/// Credential copying may reuse only a workspace the caller named explicitly
/// with `--vm`. An implicit pre-existing `--workspace-name` still requires a
/// fresh workspace so DevDock never silently changes its user credentials.
fn validate_git_credential_copy_workspace_reuse(
    copy_git_creds: bool,
    workspace_existed: bool,
    explicitly_selected_vm: bool,
) -> Result<()> {
    if copy_git_creds && workspace_existed && !explicitly_selected_vm {
        return Err(anyhow!(
            "--copy-git-creds requires a newly created Coder workspace or an explicitly selected existing --vm workspace"
        ));
    }
    Ok(())
}

struct RemoteStartLogContext<'a> {
    profile: &'a str,
    current_branch: &'a str,
    target_branch: &'a str,
    requested_checkout_oid: &'a str,
    workspace_name: &'a str,
    region: &'a str,
    offset: u16,
    auto_forward: bool,
    copy_git_creds: bool,
    verbose: bool,
}

fn log_remote_start_configuration(
    logger: &OperationLogger,
    context: &RemoteStartLogContext<'_>,
) -> Result<()> {
    logger.log_section("Configuration")?;
    logger.log(&format!("Profile: {}", context.profile))?;
    logger.log(&format!("Local Branch: {}", context.current_branch))?;
    logger.log(&format!("Target Branch: {}", context.target_branch))?;
    logger.log(&format!("Requested local checkout OID: {}", context.requested_checkout_oid))?;
    logger.log(&format!("Workspace: {}", context.workspace_name))?;
    logger.log(&format!("Region: {}", context.region))?;
    logger.log(&format!("Port Base: {}", context.offset))?;
    logger.log(&format!("Auto-forward: {}", context.auto_forward))?;
    logger.log(&format!("Copy Git credentials: {}", context.copy_git_creds))?;
    logger.log(&format!("Verbose: {}", context.verbose))?;
    Ok(())
}

fn set_startup_evidence(
    current_stage: &std::sync::Arc<std::sync::Mutex<&'static str>>,
    last_evidence: &std::sync::Arc<std::sync::Mutex<String>>,
    stage: &'static str,
    evidence: String,
) {
    *current_stage.lock().unwrap() = stage;
    *last_evidence.lock().unwrap() = evidence;
}

fn startup_failure_reason(last_evidence: &str, error: &str) -> String {
    format!("last_evidence={last_evidence}; error={error}")
}

async fn persist_post_claim_failure(
    state: &StateManager,
    instance_id: &str,
    stage: &str,
    last_evidence: &str,
    error: &str,
) -> String {
    let reason = startup_failure_reason(last_evidence, error);
    if let Err(status_error) =
        state.record_failure(instance_id, InstanceStatus::Stopped, stage, &reason).await
    {
        warn!(%status_error, instance_id, stage, "Failed to persist post-claim remote startup failure");
    }
    reason
}

async fn fail_post_claim<T>(
    state: &StateManager,
    instance_id: &str,
    stage: &str,
    last_evidence: &str,
    error: anyhow::Error,
) -> Result<T> {
    let reason =
        persist_post_claim_failure(state, instance_id, stage, last_evidence, &error.to_string())
            .await;
    Err(anyhow!(reason))
}

/// Start remote DevDock workflow.
pub async fn start_remote(config: RemoteStartConfig) -> Result<()> {
    info!("Starting remote DevDock workflow");

    let command_start_time = chrono::Utc::now();
    let wall_start = std::time::Instant::now();

    // The selected profile, not ambient gcloud configuration, is the source of
    // truth for this remote run. Load it before any session claim, Coder command,
    // SSH operation, or credential transfer.
    let profile_path = find_profile(&config.profile)?;
    let loader = ProfileLoader::new();
    let loaded_profile = loader.load_from_path(&profile_path)?;
    let profile_project = loaded_profile.project.trim();
    let managed_profile_path =
        devdock_remote::config_sync::remote_profile_repo_path(&profile_path)?;

    if let Err(error) = check_gcloud_preflight(profile_project) {
        print_local_gcloud_preflight_failure(&config.profile, profile_project, &error);
        return Err(anyhow!(error.to_string()));
    }
    let auth_check_secs = wall_start.elapsed().as_secs_f64();

    let explicitly_selected_vm = config.vm.is_some();
    if config.copy_git_creds {
        // This is a local-only `gh` preflight for both new and explicitly
        // selected reused workspaces. Remote authentication is never interactive.
        devdock_remote::config_sync::check_local_git_credentials()?;
    }

    let RemoteStartConfig {
        profile,
        workspace_name,
        branch,
        region,
        port_base,
        disk_size,
        vm,
        auto_forward,
        copy_git_creds,
        services,
        verbose,
        log_level,
    } = config;

    // Get current git info
    let worktree = get_current_worktree()?;
    let current_branch = get_current_branch()?;

    // Resolve the requested local source once, before any remote mutation. Its
    // exact local OID is the immutable checkout target for both new and reused
    // workspaces; a moving origin branch tip is not a substitute for the
    // caller's local source candidate.
    let (branch_for_workspace, use_branch_mode) = if let Some(target_branch) = &branch {
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
    let requested_checkout_oid =
        resolve_requested_checkout_oid(branch.as_deref(), &branch_for_workspace)?;

    // Determine workspace name and sanitize it (replace slashes and special chars)
    let workspace_name =
        workspace_name.or_else(|| vm.clone()).unwrap_or_else(|| current_branch.clone());
    let workspace_name = sanitize_workspace_name(&workspace_name);

    // Complete local preflight before creating an active session record.
    check_coder_installed()?;
    let identity_lookup_name = vm.as_deref().unwrap_or(&workspace_name);
    let preflight_coder_identity = coder::get_workspace_identity(identity_lookup_name)?;
    if vm.is_some() && preflight_coder_identity.is_none() {
        return Err(anyhow!(
            "Coder workspace '{identity_lookup_name}' was not found during preflight"
        ));
    }

    // Transactionally claim or reuse the session before remote mutation. Retries for the same
    // worktree/workspace pair retain the original DevDock ID and port allocation.
    let state = StateManager::new().await?;
    let claimed = state
        .claim_remote_session(RemoteSessionRequest {
            proposed_id: Instance::generate_id(SessionOrigin::Dev),
            workspace_path: PathBuf::from(&worktree),
            planned_workspace_name: workspace_name.clone(),
            branch: branch_for_workspace.clone(),
            profile: profile.clone(),
            requested_port_base: port_base,
            created_at: now(),
        })
        .await?;
    if claimed.reused {
        info!("Reusing reconciled remote session {}", claimed.instance.id);
    }
    let instance_id = claimed.instance.id;
    let offset = claimed.instance.port_base;

    // Track total time
    let overall_start = Instant::now();

    let initial_evidence = "session claimed; creating the per-session operation log";
    let logger = match OperationLogger::new(&instance_id) {
        Ok(logger) => logger,
        Err(error) => {
            return fail_post_claim(
                &state,
                &instance_id,
                stage::OPERATION_LOG,
                initial_evidence,
                error,
            )
            .await;
        }
    };
    if let Err(error) = log_remote_start_configuration(
        &logger,
        &RemoteStartLogContext {
            profile: &profile,
            current_branch: &current_branch,
            target_branch: &branch_for_workspace,
            requested_checkout_oid: &requested_checkout_oid,
            workspace_name: &workspace_name,
            region: &region,
            offset,
            auto_forward,
            copy_git_creds,
            verbose,
        },
    ) {
        return fail_post_claim(
            &state,
            &instance_id,
            stage::OPERATION_LOG,
            "operation log created; writing initial configuration evidence",
            error,
        )
        .await;
    }

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
    let t_last_evidence: std::sync::Arc<std::sync::Mutex<String>> =
        std::sync::Arc::new(std::sync::Mutex::new(format!(
            "operation log initialized at {}; preparing workspace setup",
            logger.path().display()
        )));
    // Tracks which stage is currently executing; on error, this identifies failed_at_stage.
    let t_current_stage: std::sync::Arc<std::sync::Mutex<&'static str>> =
        std::sync::Arc::new(std::sync::Mutex::new(stage::WORKSPACE_PROVISION));

    // Execute main setup logic with error handling to update status on failure
    let setup_result = async {
        // Create or start workspace
        let (workspace_name, workspace_existed_flag, known_coder_identity) = if let Some(existing) = vm
        {
            set_startup_evidence(
                &t_current_stage,
                &t_last_evidence,
                stage::WORKSPACE_PROVISION,
                format!("starting explicitly selected Coder workspace {existing}"),
            );
            logger.log_section("Using Existing Workspace")?;
            logger.log(&format!("Workspace: {existing}"))?;
            output::info(&format!("Using existing workspace: {existing}"));
            let identity = preflight_coder_identity
                .ok_or_else(|| anyhow!("Coder workspace '{existing}' disappeared after preflight"))?;
            coder::start_workspace(&existing)?;
            logger.log("Workspace started successfully")?;
            (existing, true, Some(identity))
        } else {
            // Check if workspace already exists
            logger.log(&format!("Checking if workspace '{workspace_name}' exists..."))?;
            let existing_identity = preflight_coder_identity;
            let exists = existing_identity.is_some();
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
                    println!("  1) Continue with existing workspace if git state is safe (clean and restart DevDock)");
                    println!("  2) Delete workspace and create fresh");
                    println!("  3) Cancel");
                    println!();
                } else {
                    println!("  1) Continue with existing if safe (clean and restart)");
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
                        set_startup_evidence(
                            &t_current_stage,
                            &t_last_evidence,
                            stage::WORKSPACE_PROVISION,
                            format!("starting existing Coder workspace {workspace_name}"),
                        );
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

                        (workspace_name.clone(), true, existing_identity)
                    }
                    "2" => {
                        set_startup_evidence(
                            &t_current_stage,
                            &t_last_evidence,
                            stage::WORKSPACE_PROVISION,
                            format!("recreating Coder workspace {workspace_name}"),
                        );
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
                        (ws_name, false, None)
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
                set_startup_evidence(
                    &t_current_stage,
                    &t_last_evidence,
                    stage::WORKSPACE_PROVISION,
                    format!("creating Coder workspace {workspace_name}"),
                );
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
                (ws_name, false, None)
            }
        };
        let workspace_ready_secs = wall_start.elapsed().as_secs_f64();
        *t_workspace_ready.lock().unwrap() = Some(workspace_ready_secs);
        *t_workspace_existed.lock().unwrap() = workspace_existed_flag;

        set_startup_evidence(
            &t_current_stage,
            &t_last_evidence,
            stage::WORKSPACE_IDENTITY,
            format!("resolving immutable Coder identity for {workspace_name}"),
        );
        let coder_identity = if let Some(identity) = known_coder_identity {
            identity
        } else {
            coder::get_workspace_identity(&workspace_name)?.ok_or_else(|| {
                anyhow!(
                    "Coder workspace '{workspace_name}' was not returned after it became reachable"
                )
            })?
        };
        // Coder accepts owner-qualified names for lookup and startup, but process fingerprints and
        // durable session identity must use the canonical name returned by the API. Shadow the
        // requested spelling before any managed forwards are created.
        let workspace_name = coder_identity.name.clone();
        validate_git_credential_copy_workspace_reuse(
            copy_git_creds,
            workspace_existed_flag,
            explicitly_selected_vm,
        )?;

        // A clean checkout may be safely converged even when its branch has
        // moved. The only exception is the exact selected profile path, whose
        // unstaged residue is DevDock-managed and is restored by the remote
        // checkout script. Any other tracked, staged, or untracked entry is a
        // hard no-mutation boundary. This runs before cleanup, configuration
        // transfer, sync, or remote DevDock startup.
        set_startup_evidence(
            &t_current_stage,
            &t_last_evidence,
            stage::WORKSPACE_CHECKOUT,
            format!("reconciling remote checkout for {workspace_name} to {requested_checkout_oid}"),
        );
        let checkout = ensure_remote_workspace_checkout(
            &workspace_name,
            &branch_for_workspace,
            &requested_checkout_oid,
            &managed_profile_path,
            Some(logger.path()),
        )?;
        logger.log(&format!(
            "Remote checkout reconciled: branch={}, requested_oid={}, observed_oid={}",
            checkout.branch, requested_checkout_oid, checkout.head_oid
        ))?;

        state
            .attach_remote_workspace(&instance_id, &coder_identity.id, &workspace_name)
            .await?;
        set_startup_evidence(
            &t_current_stage,
            &t_last_evidence,
            stage::CONFIG_SYNC,
            format!("syncing configuration and credentials to {workspace_name}"),
        );

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
            workspace_name
        ))?;

        // Step 1: configuration and GCP credentials are complete before any
        // source-sync attempt. The GCP path invokes Scio's helper and then
        // verifies remote auth/ADC/project noninteractively.
        let config_start = Instant::now();
        let spinner_config = if verbose {
            output::info("Syncing configuration files to remote...");
            None
        } else {
            Some(output::create_spinner("Config"))
        };
        workspace::sync_remote_configuration(
            &workspace_name,
            &profile_path,
            profile_project,
            copy_git_creds,
            Some(logger.path()),
            verbose,
        )?;
        *t_config_sync.lock().unwrap() = Some(wall_start.elapsed().as_secs_f64());
        if let Some(spinner) = spinner_config {
            spinner.finish_with_message(format!(
                "Config {}",
                format!("✓ {}s", config_start.elapsed().as_secs()).green()
            ));
        }

        // Step 2: source sync must establish a fresh, successful initial scan
        // before DevDock/Tilt can be launched. A PID alone is not evidence.
        set_startup_evidence(
            &t_current_stage,
            &t_last_evidence,
            stage::SYNC_DAEMON,
            format!("starting initial source-sync watermark for {workspace_name}"),
        );
        let sync_start = Instant::now();
        let spinner_sync = Some(output::create_spinner("Sync"));
        let repo_path = PathBuf::from(&worktree);
        if let Some(pid) = sync_daemon::read_daemon_pid(&instance_id)? {
            if sync_daemon::is_daemon_running(&instance_id, pid) {
                logger.log(&format!(
                    "Stopping owned sync daemon PID {pid} so this retry has a fresh initial scan"
                ))?;
            }
            // `stop_daemon` only signals a PID after its command and DevDock ID
            // prove ownership; it waits for that daemon to exit before deleting
            // exact-ID evidence files.
            sync_daemon::stop_daemon(&instance_id)?;
        }
        let sync_pid = sync_daemon::start_daemon(
            &instance_id,
            &workspace_name,
            &repo_path,
            false,
            use_branch_mode,
            &log_level,
            &loaded_profile.sync.force_patterns,
            devdock_sync::universal_matcher(),
        )?;
        state
            // The daemon is already live; `starting` is a coordinator phase, not a
            // persisted sync-session status. Keep the database value within its
            // running/stopped/error constraint while the initial watermark is verified.
            .save_sync_session(&instance_id, &workspace_name, Some(sync_pid), false, "running")
            .await?;
        let watermark = sync_daemon::wait_for_initial_sync_watermark(
            &instance_id,
            &workspace_name,
            &repo_path,
            sync_pid,
            std::time::Duration::from_secs(120),
        )?;
        let recorded_pid = sync_daemon::read_daemon_pid(&instance_id)?;
        if recorded_pid != Some(sync_pid) || !sync_daemon::is_daemon_running(&instance_id, sync_pid)
        {
            return Err(anyhow!("sync daemon PID/state does not match the initial watermark"));
        }
        state
            .save_sync_session(&instance_id, &workspace_name, Some(sync_pid), false, "running")
            .await?;
        *t_sync_daemon.lock().unwrap() = Some(wall_start.elapsed().as_secs_f64());
        if let Some(spinner) = spinner_sync {
            spinner.finish_with_message(format!(
                "Sync {}",
                format!("✓ {}s", sync_start.elapsed().as_secs()).green()
            ));
        }
        logger.log(&format!(
            "Initial sync watermark: workspace={}, local_head={}, completed_at={}",
            watermark.workspace_name,
            watermark.local_head_oid.as_deref().unwrap_or("unavailable"),
            watermark.completed_at
        ))?;

        // Step 3: only after the sync gate can the remote process and its owned
        // managed forward be created.
        set_startup_evidence(
            &t_current_stage,
            &t_last_evidence,
            stage::REMOTE_DEVDOCK_START,
            format!("starting remote DevDock process in {workspace_name}"),
        );
        let start_time = Instant::now();
        let spinner_start = if verbose { None } else { Some(output::create_spinner("Starting")) };
        let port_mgr = workspace::start_remote_devdock(
            &state,
            &workspace_name,
            &coder_identity.id,
            &profile,
            &profile_path,
            &instance_id,
            services.as_deref(),
            auto_forward,
            offset,
            Some(logger.path()),
            verbose,
        )
        .await?;
        if let Some(spinner) = spinner_start {
            spinner.finish_with_message(format!(
                "Starting {}",
                format!("✓ {}s", start_time.elapsed().as_secs()).green()
            ));
        }
        logger.log("DevDock process and managed forward started")?;

        *t_current_stage.lock().unwrap() = stage::PORT_FORWARD;
        if auto_forward && port_mgr.is_some() {
            *t_port_forward.lock().unwrap() = Some(wall_start.elapsed().as_secs_f64());
        }

        // A tmux session and open port are necessary but insufficient. Require
        // the owned remote identity plus two strict Tilt API snapshots before
        // state can become Running or output can claim Ready.
        set_startup_evidence(
            &t_current_stage,
            &t_last_evidence,
            stage::TILT_READINESS,
            format!("verifying strict Tilt readiness for {workspace_name}"),
        );
        let expected_resources = expected_tilt_resources(&loaded_profile, services.as_deref());
        verify_remote_tilt_readiness(
            &workspace_name,
            &instance_id,
            offset + 10350,
            &expected_resources,
            Some(logger.path()),
        )?;
        *t_tilt_up.lock().unwrap() = Some(wall_start.elapsed().as_secs_f64());
        logger.log(&format!(
            "Tilt readiness gate passed: expected_resources={}",
            expected_resources.join(",")
        ))?;

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
                    "devdock ports forward --all --no-offset".yellow()
                );
                println!(
                    "         {}",
                    "This includes nginx so local.glean.com routes work.".dimmed()
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
                    "devdock ports forward --all --no-offset".cyan(),
                    "(forward service ports including nginx)".dimmed()
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
            let last_evidence = t_last_evidence.lock().unwrap().clone();
            let failure_reason = startup_failure_reason(&last_evidence, &e.to_string());

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
                Some(&failure_reason),
            );
            emit_remote_telemetry(metrics).await;

            // Preserve the failed stage/reason while retaining the retryable session identity.
            info!("Setup failed, recording retryable session failure");
            if let Err(status_err) = state
                .record_failure(
                    &instance_id,
                    InstanceStatus::Stopped,
                    failed_stage,
                    &failure_reason,
                )
                .await
            {
                warn!("Failed to update instance status: {}", status_err);
            }

            if let Err(log_error) = logger.log_section("Setup Failed") {
                warn!(%log_error, "Could not record remote setup failure in operation log");
            }
            if let Err(log_error) = logger.log(&format!("Error: {failure_reason}")) {
                warn!(%log_error, "Could not record remote setup failure in operation log");
            }
            if let Err(log_error) = logger
                .log(&format!("Instance {} marked as Stopped", Instance::display_id(&instance_id)))
            {
                warn!(%log_error, "Could not record remote setup failure in operation log");
            }

            output::error(&format!("DevDock setup failed: {e}"));
            print_failed_start_recovery(
                &profile_path,
                &workspace_name,
                offset,
                services.as_deref(),
                &instance_id,
                Some(&branch_for_workspace),
            );
            println!();
            println!(
                "To clean up verified local state: devdock stop {}",
                Instance::display_id(&instance_id)
            );
            Err(e)
        }
    }
}

/// Print additive recovery guidance after remote startup fails.
///
/// The commands are deliberately raw `coder port-forward` invocations. They
/// are not registered as managed forwards and are never adopted or killed by
/// DevDock, which preserves the #240 ownership boundary.
fn print_failed_start_recovery(
    profile_path: &std::path::Path,
    workspace_name: &str,
    offset: u16,
    services: Option<&str>,
    instance_id: &str,
    requested_revision: Option<&str>,
) {
    println!();
    println!("{}", "Remote start recovery".bold());
    println!("  If DevDock is running in Coder, attach the stopped local session with:",);
    println!("    devdock recover --workspace {workspace_name}");

    match PortForwardManager::manual_forward_commands(
        profile_path,
        workspace_name,
        offset,
        services,
    ) {
        Ok(commands) if !commands.is_empty() => {
            println!("  Raw forwarding commands for this start:");
            for command in commands {
                println!("    {command}");
            }
        }
        Ok(_) => {}
        Err(error) => {
            println!(
                "  Could not resolve selected service ports locally ({error}); forward Tilt manually:"
            );
            println!(
                "    {}",
                coder::port_forward_command_line(workspace_name, offset + 10350, 10350)
            );
        }
    }

    match inspect_remote_runtime(workspace_name, None) {
        Ok(snapshot) if snapshot.has_surviving_containers() => {
            println!();
            print_stale_container_warning(
                &snapshot,
                workspace_name,
                instance_id,
                requested_revision,
            );
        }
        Ok(_) => {}
        Err(_) => {
            // The workspace may not have been created or may be unreachable;
            // do not turn best-effort diagnostics into a second failure.
        }
    }
}

pub(crate) fn print_stale_container_warning(
    snapshot: &devdock_remote::RemoteRuntimeSnapshot,
    workspace_name: &str,
    instance_id: &str,
    requested_revision: Option<&str>,
) {
    println!("{}", "⚠️  STALE CONTAINER WARNING".yellow().bold());
    println!(
        "  Remote Docker still has service containers: {}",
        snapshot.docker_containers.join(", ")
    );
    println!("  These surviving containers are from a prior run and are not valid test targets.");
    println!(
        "  Current session/Tilt identity is not trusted until recovery verifies tmux, Tilt, and DEVDOCK_ID."
    );
    println!("  Expected local session: {}", Instance::display_id(instance_id));
    if let Some(revision) = requested_revision {
        println!("  Requested Git revision/branch: {revision}");
    }
    if let Some(remote_id) = &snapshot.devdock_id {
        println!("  Observed remote session: {remote_id}");
    }
    if let Some(revision) = &snapshot.git_revision {
        println!("  Observed remote Git revision: {revision}");
    }
    if let Some(branch) = &snapshot.git_branch {
        println!("  Observed remote Git branch: {branch}");
    }
    println!("  No automatic cleanup was performed because container ownership is not proven.");
    println!(
        "  Supported cleanup/retry: coder ssh {workspace_name} -- 'cd /home/workspace/scio && devdock cleanup --soft'"
    );
    println!("  Then retry: devdock recover --workspace {workspace_name}");
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExistingWorkspaceGuardIssue {
    Dirty { entries: Vec<String> },
}

fn evaluate_existing_workspace_guard(
    state: &workspace::RemoteGitState,
    managed_profile_path: &str,
) -> Vec<ExistingWorkspaceGuardIssue> {
    let mut issues = Vec::new();

    if state.is_dirty() && !has_only_unstaged_managed_profile_residue(state, managed_profile_path) {
        issues.push(ExistingWorkspaceGuardIssue::Dirty { entries: state.dirty_entries.clone() });
    }

    issues
}

fn has_only_unstaged_managed_profile_residue(
    state: &workspace::RemoteGitState,
    managed_profile_path: &str,
) -> bool {
    state.dirty_entries.as_slice() == [format!(" M {managed_profile_path}")]
}

fn ensure_remote_workspace_checkout(
    workspace_name: &str,
    expected_branch: &str,
    expected_oid: &str,
    managed_profile_path: &str,
    log_file: Option<&std::path::Path>,
) -> Result<workspace::RemoteGitState> {
    let state = workspace::inspect_remote_git_state(workspace_name, log_file)
        .map_err(|e| anyhow!("Failed to inspect remote workspace git state: {e}"))?;

    let issues = evaluate_existing_workspace_guard(&state, managed_profile_path);
    if issues.is_empty() {
        return workspace::reconcile_remote_checkout(
            workspace_name,
            expected_branch,
            expected_oid,
            Some(managed_profile_path),
            log_file,
        )
        .map_err(|error| anyhow!("Failed to reconcile remote workspace checkout: {error}"));
    }

    output::error("Refusing to continue with existing remote workspace");
    println!();
    println!("Remote workspace: {}", workspace_name.cyan());
    println!("Expected branch:  {}", expected_branch.cyan());
    println!("Expected OID:     {}", expected_oid.cyan());
    println!("Remote branch:    {}", state.branch.yellow());
    println!("Remote OID:       {}", state.head_oid.yellow());
    println!();

    for issue in &issues {
        match issue {
            ExistingWorkspaceGuardIssue::Dirty { entries } => {
                println!("{} Remote git tree has uncommitted/untracked changes:", "•".yellow());
                for entry in entries.iter().take(10) {
                    println!("    {}", entry.yellow());
                }
                if entries.len() > 10 {
                    println!("    ... and {} more", entries.len() - 10);
                }
            }
        }
    }

    println!();
    println!("To inspect and resolve:");
    println!("  {}", format!("coder ssh {workspace_name}").cyan());
    println!("  {}", "cd /home/workspace/scio && git status --short --branch".cyan());
    println!();
    println!("Resolve the remote git state before retrying. DevDock will not stash or reset it.");

    Err(anyhow!("Existing remote workspace '{workspace_name}' is not safe to continue"))
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

/// Resolve the exact local OID selected for remote startup.
///
/// The remote workflow must never auto-push a local branch as part of startup.
/// It verifies that the checkout branch is published, then the remote checkout
/// fetches the branch and proves this local commit object is available before any
/// reset. Without an explicit `--branch`, the source is the actual worktree
/// `HEAD`, including a detached HEAD.
fn resolve_requested_checkout_oid(
    explicit_branch: Option<&str>,
    checkout_branch: &str,
) -> Result<String> {
    ensure_branch_published_at_origin(checkout_branch)?;
    resolve_local_revision_oid(&local_checkout_revision(explicit_branch, checkout_branch))
}

fn local_checkout_revision(explicit_branch: Option<&str>, checkout_branch: &str) -> String {
    if explicit_branch.is_some() {
        format!("refs/heads/{checkout_branch}^{{commit}}")
    } else {
        "HEAD^{commit}".to_string()
    }
}

fn ensure_branch_published_at_origin(branch: &str) -> Result<()> {
    use std::process::Command;

    let remote_ref = format!("refs/heads/{branch}");
    let remote_check =
        Command::new("git").args(["ls-remote", "--exit-code", "origin", &remote_ref]).output()?;

    if !remote_check.status.success() {
        return Err(anyhow!(
            "Branch '{}' is not available at origin. Publish it before starting remote DevDock.",
            branch
        ));
    }

    Ok(())
}

fn resolve_local_revision_oid(revision: &str) -> Result<String> {
    use std::process::Command;

    let local_check = Command::new("git")
        .args(["rev-parse", "--verify", "--end-of-options", revision])
        .output()?;
    if !local_check.status.success() {
        return Err(anyhow!(
            "Local source revision '{revision}' is not available. Check it out or fetch it locally before starting remote DevDock."
        ));
    }

    parse_local_revision_oid(revision, &String::from_utf8_lossy(&local_check.stdout))
}

fn parse_local_revision_oid(revision: &str, stdout: &str) -> Result<String> {
    let mut matches = stdout.lines().map(str::trim).filter(|line| !line.is_empty());
    let oid = matches.next().ok_or_else(|| {
        anyhow!("local source revision '{revision}' did not resolve to an exact OID")
    })?;
    if matches.next().is_some()
        || !matches!(oid.len(), 40 | 64)
        || !oid.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(anyhow!(
            "local source revision '{revision}' resolved to an invalid or ambiguous OID"
        ));
    }

    Ok(oid.to_string())
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

/// Render the only remediation for an unusable local gcloud session.
///
/// Keep this in the CLI rather than the remote crate so the preflight remains
/// testable and cannot accidentally launch an interactive command. In
/// particular, do not add a remote-login alternative here.
fn print_local_gcloud_preflight_failure(profile: &str, project: &str, error: &RemoteError) {
    match error {
        RemoteError::GcloudNotInstalled => {
            output::error("gcloud CLI not found");
            println!("Install gcloud: {}", "https://cloud.google.com/sdk/docs/install".cyan());
        }
        RemoteError::GcloudPreflightFailed { .. } => {
            output::error("local gcloud preflight failed before remote startup");
            println!("Profile: {profile}");
            println!("Project: {project}");
            println!("Run this locally, then rerun the same DevDock command:");
            println!("gcloud auth login --update-adc");
        }
        RemoteError::ConfigSyncError(message) if message.contains("no usable project") => {
            output::error("selected profile has no usable project");
            println!("Profile: {profile}");
        }
        _ => output::error(&error.to_string()),
    }
}

fn expected_tilt_resources(profile: &Profile, services: Option<&str>) -> Vec<String> {
    let mut resources = match services {
        Some(services) => services
            .split(',')
            .map(str::trim)
            .filter(|service| !service.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>(),
        None => profile.all_components(),
    };
    resources.sort();
    resources.dedup();
    resources
}

/// Require the managed forward to expose one stable, identity-checked Tilt
/// session. `TiltClient` rejects an HTML response, wrong content type/schema,
/// missing session cookie, and invalid `tiltStartTime`; this adds the remote
/// session and selected-resource checks that are specific to remote startup.
fn verify_remote_tilt_readiness(
    workspace_name: &str,
    instance_id: &str,
    tilt_port: u16,
    expected_resources: &[String],
    log_file: Option<&std::path::Path>,
) -> Result<()> {
    if expected_resources.is_empty() {
        return Err(anyhow!(
            "Tilt readiness cannot determine any selected resources from the profile"
        ));
    }

    let client = TiltClient::new(format!("http://127.0.0.1:{tilt_port}"))?;
    let deadline = Instant::now() + std::time::Duration::from_secs(120);
    let mut last_error = None;

    while Instant::now() < deadline {
        let runtime = workspace::inspect_remote_runtime(workspace_name, log_file)
            .map_err(|error| anyhow!("could not inspect remote Tilt ownership: {error}"))?;
        if !runtime.matches_session(instance_id) {
            return Err(anyhow!("remote tmux/Tilt identity does not match this DevDock attempt"));
        }

        match client.get_snapshot() {
            Ok(first) => {
                if !snapshot_has_ready_selected_resources(&first.resources, expected_resources) {
                    last_error =
                        Some("selected Tilt resources are missing, pending, or failed".to_string());
                } else {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    let second_runtime =
                        workspace::inspect_remote_runtime(workspace_name, log_file).map_err(
                            |error| anyhow!("could not recheck remote Tilt ownership: {error}"),
                        )?;
                    if !second_runtime.matches_session(instance_id) {
                        return Err(anyhow!(
                            "remote tmux/Tilt identity changed during readiness verification"
                        ));
                    }
                    match client.get_snapshot() {
                        Ok(second) => {
                            first.ensure_same_tilt_session(&second)?;
                            if snapshot_has_ready_selected_resources(
                                &second.resources,
                                expected_resources,
                            ) {
                                return Ok(());
                            }
                            last_error = Some(
                                "selected Tilt resources changed, are missing, pending, or failed"
                                    .to_string(),
                            );
                        }
                        Err(error) => last_error = Some(error.to_string()),
                    }
                }
            }
            Err(error) => last_error = Some(error.to_string()),
        }

        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    Err(anyhow!(
        "Timed out waiting for strict Tilt readiness: {}",
        last_error.unwrap_or_else(|| "no valid Tilt snapshot".to_string())
    ))
}

fn snapshot_has_ready_selected_resources(
    resources: &[devdock_monitor::TiltResource],
    expected_resources: &[String],
) -> bool {
    expected_resources.iter().all(|expected| {
        resources
            .iter()
            .find(|resource| resource.name == *expected)
            .is_some_and(|resource| resource.status.is_healthy())
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_allows_clean_checkout_regardless_of_prior_branch() {
        let state = workspace::RemoteGitState {
            branch: "previous-branch".to_string(),
            head_oid: "0123456789abcdef0123456789abcdef01234567".to_string(),
            dirty_entries: vec![],
        };

        assert!(evaluate_existing_workspace_guard(&state, "devdock/profiles/qe_smoke_test.yaml")
            .is_empty());
    }

    #[test]
    fn guard_detects_dirty_state() {
        let state = workspace::RemoteGitState {
            branch: "feature/foo".to_string(),
            head_oid: "0123456789abcdef0123456789abcdef01234567".to_string(),
            dirty_entries: vec![" M java/Foo.java".to_string()],
        };

        let issues =
            evaluate_existing_workspace_guard(&state, "devdock/profiles/qe_smoke_test.yaml");
        assert_eq!(
            issues,
            vec![ExistingWorkspaceGuardIssue::Dirty {
                entries: vec![" M java/Foo.java".to_string()],
            }]
        );
    }

    #[test]
    fn guard_refuses_untracked_state() {
        let state = workspace::RemoteGitState {
            branch: "main".to_string(),
            head_oid: "0123456789abcdef0123456789abcdef01234567".to_string(),
            dirty_entries: vec!["?? new_file.py".to_string()],
        };

        let issues =
            evaluate_existing_workspace_guard(&state, "devdock/profiles/qe_smoke_test.yaml");
        assert_eq!(
            issues,
            vec![ExistingWorkspaceGuardIssue::Dirty {
                entries: vec!["?? new_file.py".to_string()]
            }]
        );
    }

    #[test]
    fn guard_allows_only_the_selected_unstaged_profile_residue() {
        let selected = "devdock/profiles/qe_smoke_test.yaml";
        let state = workspace::RemoteGitState {
            branch: "main".to_string(),
            head_oid: "0123456789abcdef0123456789abcdef01234567".to_string(),
            dirty_entries: vec![format!(" M {selected}")],
        };

        assert!(evaluate_existing_workspace_guard(&state, selected).is_empty());
    }

    #[test]
    fn guard_refuses_unknown_staged_and_untracked_dirty_paths() {
        let selected = "devdock/profiles/qe_smoke_test.yaml";
        for dirty_entries in [
            vec![" M devdock/profiles/other.yaml".to_string()],
            vec![format!("M  {selected}")],
            vec![format!("?? {selected}")],
            vec![format!(" M {selected}"), " M java/Foo.java".to_string()],
        ] {
            let state = workspace::RemoteGitState {
                branch: "main".to_string(),
                head_oid: "0123456789abcdef0123456789abcdef01234567".to_string(),
                dirty_entries,
            };

            assert!(!evaluate_existing_workspace_guard(&state, selected).is_empty());
        }
    }

    #[test]
    fn parses_only_the_exact_local_source_oid() {
        let oid =
            parse_local_revision_oid("HEAD^{commit}", "0123456789abcdef0123456789abcdef01234567\n")
                .unwrap();

        assert_eq!(oid, "0123456789abcdef0123456789abcdef01234567");
        assert!(parse_local_revision_oid(
            "HEAD^{commit}",
            "0123456789abcdef0123456789abcdef01234567\n0123456789abcdef0123456789abcdef01234567\n"
        )
        .is_err());
    }

    #[test]
    fn implicit_checkout_uses_detached_worktree_head_while_explicit_branch_uses_its_local_ref() {
        assert_eq!(local_checkout_revision(None, "master"), "HEAD^{commit}");
        assert_eq!(
            local_checkout_revision(Some("feature/reused"), "feature/reused"),
            "refs/heads/feature/reused^{commit}"
        );
    }

    #[test]
    fn copy_git_creds_allows_an_explicit_vm_workspace_reuse() {
        validate_git_credential_copy_workspace_reuse(true, true, true)
            .expect("an explicitly selected --vm workspace may receive opt-in credentials");
    }

    #[test]
    fn copy_git_creds_rejects_implicit_existing_workspace_reuse() {
        let error = validate_git_credential_copy_workspace_reuse(true, true, false)
            .expect_err("an implicit existing workspace must not receive credentials");

        assert!(error.to_string().contains("explicitly selected existing --vm workspace"));
    }

    #[tokio::test]
    async fn post_claim_operation_log_failure_is_durable_and_retryable() {
        let state_dir = tempfile::tempdir().expect("state directory");
        let worktree = tempfile::tempdir().expect("worktree");
        let manager = StateManager::with_path(&state_dir.path().join("devdock.db"))
            .await
            .expect("state manager");
        let claimed = manager
            .claim_remote_session(RemoteSessionRequest {
                proposed_id: "dev-operation-log".to_string(),
                workspace_path: worktree.path().to_path_buf(),
                planned_workspace_name: "workspace".to_string(),
                branch: "main".to_string(),
                profile: "qe_smoke_test".to_string(),
                requested_port_base: None,
                created_at: 10,
            })
            .await
            .expect("claim session");

        let error = fail_post_claim::<()>(
            &manager,
            &claimed.instance.id,
            stage::OPERATION_LOG,
            "session claimed; creating the per-session operation log",
            anyhow!("permission denied"),
        )
        .await
        .expect_err("a post-claim logger failure must still fail startup");
        let reason = error.to_string();
        let failed = manager
            .get_instance(&claimed.instance.id)
            .await
            .expect("read failed session")
            .expect("claimed session exists");

        assert_eq!(failed.status, InstanceStatus::Stopped);
        assert_eq!(failed.failure_stage.as_deref(), Some(stage::OPERATION_LOG));
        assert_eq!(failed.failure_reason.as_deref(), Some(reason.as_str()));
        assert!(reason.contains("last_evidence=session claimed"));

        let retry = manager
            .claim_remote_session(RemoteSessionRequest {
                proposed_id: "dev-new".to_string(),
                workspace_path: worktree.path().to_path_buf(),
                planned_workspace_name: "workspace".to_string(),
                branch: "main".to_string(),
                profile: "qe_smoke_test".to_string(),
                requested_port_base: None,
                created_at: 20,
            })
            .await
            .expect("retry claims the same session");
        assert_eq!(retry.instance.id, claimed.instance.id);
    }

    #[tokio::test]
    async fn post_claim_startup_timeout_records_stage_and_last_evidence() {
        let state_dir = tempfile::tempdir().expect("state directory");
        let worktree = tempfile::tempdir().expect("worktree");
        let manager = StateManager::with_path(&state_dir.path().join("devdock.db"))
            .await
            .expect("state manager");
        let claimed = manager
            .claim_remote_session(RemoteSessionRequest {
                proposed_id: "dev-timeout".to_string(),
                workspace_path: worktree.path().to_path_buf(),
                planned_workspace_name: "workspace".to_string(),
                branch: "main".to_string(),
                profile: "qe_smoke_test".to_string(),
                requested_port_base: None,
                created_at: 10,
            })
            .await
            .expect("claim session");

        persist_post_claim_failure(
            &manager,
            &claimed.instance.id,
            stage::WORKSPACE_PROVISION,
            "starting explicitly selected Coder workspace workspace",
            "coder start exceeded the 120s remote setup command deadline",
        )
        .await;
        let failed = manager
            .get_instance(&claimed.instance.id)
            .await
            .expect("read failed session")
            .expect("claimed session exists");

        assert_eq!(failed.status, InstanceStatus::Stopped);
        assert_eq!(failed.failure_stage.as_deref(), Some(stage::WORKSPACE_PROVISION));
        assert!(failed
            .failure_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("last_evidence=starting explicitly selected")));
        assert!(failed
            .failure_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("120s remote setup command deadline")));
    }

    #[test]
    fn expected_tilt_resources_prefer_the_selected_service_subset() {
        let profile = Profile {
            project: "test-project".to_string(),
            platform: Default::default(),
            infra: Default::default(),
            services: Default::default(),
            pipelines: Default::default(),
            test_suite: Default::default(),
            env: Default::default(),
            ini_overrides: Default::default(),
            multi_project: None,
            sync: Default::default(),
        };

        assert_eq!(
            expected_tilt_resources(&profile, Some("qe, qe, frontend")),
            vec!["frontend".to_string(), "qe".to_string()]
        );
    }

    #[test]
    fn tilt_gate_requires_each_selected_resource_to_be_healthy() {
        let healthy = devdock_monitor::TiltResource {
            name: "qe".to_string(),
            labels: vec![],
            resource_type: devdock_monitor::ResourceType::Remote,
            status: devdock_monitor::ResourceStatus::Healthy,
            runtime_status: Some("ok".to_string()),
            update_status: Some("ok".to_string()),
            compose_health: None,
            is_disabled: false,
            build_start_time: None,
            build_finish_time: None,
            build_duration_secs: None,
            build_history: vec![],
        };
        let pending = devdock_monitor::TiltResource {
            name: "frontend".to_string(),
            status: devdock_monitor::ResourceStatus::Pending,
            ..healthy.clone()
        };

        assert!(snapshot_has_ready_selected_resources(
            std::slice::from_ref(&healthy),
            &["qe".to_string()]
        ));
        assert!(!snapshot_has_ready_selected_resources(
            &[healthy, pending],
            &["qe".to_string(), "frontend".to_string()]
        ));
    }
}
