//! Remote DevDock workflow implementation.

use anyhow::{anyhow, Result};
use colored::Colorize;
use devdock_remote::{coder, workspace, CoderWorkspaceConfig};
use devdock_state::{instance::{Instance, InstanceStatus}, StateManager};
use devdock_sync::daemon as sync_daemon;
use std::path::PathBuf;
use std::time::Instant;
use tracing::{info, warn};
use crate::{output, logging::OperationLogger};

/// Configuration for starting remote DevDock.
pub struct RemoteStartConfig {
    pub profile: String,
    pub workspace_name: Option<String>,
    pub region: String,
    pub port_base: Option<u16>,
    pub vm: Option<String>,
    pub auto_forward: bool,
    pub services: Option<String>,
    pub verbose: bool,
}

/// Progress reporter for cleaner output.
struct ProgressReporter {
    current_step: usize,
    total_steps: usize,
}

impl ProgressReporter {
    fn new(total_steps: usize) -> Self {
        Self {
            current_step: 0,
            total_steps,
        }
    }
    
    fn start_step(&mut self, emoji: &str, name: &str) {
        self.current_step += 1;
        print!("[{}/{}] {}  {}... ", self.current_step, self.total_steps, emoji, name);
        use std::io::{self, Write};
        io::stdout().flush().ok();
    }
    
    fn finish_step(&self, duration_secs: u64) {
        if duration_secs > 0 {
            println!("{} {}s", "✓".green(), duration_secs);
        } else {
            println!("{}", "✓".green());
        }
    }
}

/// Start remote DevDock workflow.
pub async fn start_remote(config: RemoteStartConfig) -> Result<()> {
    info!("Starting remote DevDock workflow");
    
    let RemoteStartConfig {
        profile,
        workspace_name,
        region,
        port_base,
        vm,
        auto_forward,
        services,
        verbose,
    } = config;
    
    // Get current git info
    let worktree = get_current_worktree()?;
    let branch = get_current_branch()?;
    
    // Determine workspace name and sanitize it (replace slashes and special chars)
    let workspace_name = workspace_name
        .or_else(|| vm.clone())
        .unwrap_or_else(|| branch.clone());
    let workspace_name = sanitize_workspace_name(&workspace_name);
    
    // Find profile path
    let profile_path = find_profile(&profile)?;
    
    // Register instance in database
    let state = StateManager::new().await?;
    let instance_id = uuid::Uuid::new_v4().to_string();
    let offset = calculate_port_offset(&state, port_base).await?;
    
    let instance = Instance {
        id: instance_id.clone(),
        workspace_path: worktree.clone(),
        workspace_name: Some(workspace_name.clone()),
        branch: branch.clone(),
        profile: profile.to_string(),
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
    logger.log(&format!("Profile: {}", profile))?;
    logger.log(&format!("Branch: {}", branch))?;
    logger.log(&format!("Workspace: {}", workspace_name))?;
    logger.log(&format!("Region: {}", region))?;
    logger.log(&format!("Port Base: {}", offset))?;
    logger.log(&format!("Auto-forward: {}", auto_forward))?;
    logger.log(&format!("Verbose: {}", verbose))?;
    
    // Create progress reporter (4 main steps) - only used in non-verbose mode
    let mut progress = ProgressReporter::new(4);
    
    // Display initial info (cleaner, more compact)
    println!();
    if verbose {
        output::header("Starting Remote DevDock");
        println!("  {}: {}", "Profile".cyan(), profile);
        println!("  {}: {}", "Branch".cyan(), branch);
        println!("  {}: {}", "Workspace".cyan(), workspace_name.green());
        println!("  {}: devdock-glean", "Template".cyan());
        println!("  {}: {}", "Region".cyan(), region);
        println!("  {}: {}", "DevDock ID".cyan(), &instance_id[..8]);
        println!("  {}: {}", "Port Base".cyan(), offset);
        println!();
        println!("{}", "⏱️  Expected time: ~2-3 minutes total".yellow());
        println!("   • Creating workspace: ~1-2 min");
        println!("   • Starting DevDock: ~1 min");
        println!();
        println!("📋 Session logs: {}", logger.path().display().to_string().dimmed());
        println!();
    } else {
        // Compact header
        println!("{}", "╭────────────────────────────────────────╮".cyan());
        println!("{}", format!("│  🚀 Remote DevDock: {}@{}", profile, workspace_name).cyan());
        println!("{}", "╰────────────────────────────────────────╯".cyan());
        println!();
        println!(
            "{} | {} | {}",
            format!("Profile: {}", profile).dimmed(),
            format!("Branch: {}", branch).dimmed(),
            format!("ID: {}", &instance_id[..8]).dimmed()
        );
        println!();
        println!("⏱️  Expected: ~2-3 min | Watch: {}", "devdock logs -f".cyan());
        println!();
    }
    
    // Execute main setup logic with error handling to update status on failure
    let setup_result = async {
        // Create or start workspace
        let workspace_name = if let Some(existing) = vm {
        logger.log_section("Using Existing Workspace")?;
        logger.log(&format!("Workspace: {}", existing))?;
        output::info(&format!("Using existing workspace: {}", existing));
        coder::start_workspace(&existing)?;
        logger.log("Workspace started successfully")?;
        existing
    } else {
        // Check if workspace already exists
        logger.log(&format!("Checking if workspace '{}' exists...", workspace_name))?;
        let exists = coder::workspace_exists(&workspace_name)?;
        logger.log(&format!("Workspace exists: {}", exists))?;
        
        if exists {
            if verbose {
                println!();
                output::warn(&format!("Workspace '{}' already exists!", workspace_name));
                println!();
            } else {
                println!("{}", "⚠️  Existing workspace - restarting (use --recreate to start fresh)".to_string().yellow());
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
                    logger.log(&format!("Workspace: {}", workspace_name))?;
                    logger.log("User chose to continue with existing workspace")?;
                    
                    if !verbose {
                        println!();
                    }
                    
                    // Start workspace if stopped (silent in non-verbose mode)
                    if verbose {
                        output::info(&format!("Using existing workspace: {}", workspace_name));
                    }
                    coder::start_workspace(&workspace_name)?;
                    logger.log("Workspace started")?;
                    
                    // Cleanup existing DevDock session
                    if verbose {
                        output::info("Cleaning up existing DevDock session...");
                    }
                    workspace::cleanup_existing_devdock(&workspace_name, Some(logger.path()))?;
                    logger.log("Existing DevDock session cleaned up")?;
                    
                    workspace_name.clone()
                },
                "2" => {
                    logger.log_section("Deleting and Recreating Workspace")?;
                    logger.log(&format!("Workspace: {}", workspace_name))?;
                    logger.log("User chose to delete and recreate workspace")?;
                    
                    output::info(&format!("Deleting existing workspace: {}", workspace_name));
                    coder::delete_workspace(&workspace_name)?;
                    logger.log("Workspace deleted")?;
                    
                    // Wait a moment for deletion to complete
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    
                    // Create new workspace
                    logger.log_section("Creating Workspace")?;
                    logger.log("Template: devdock-glean")?;
                    logger.log(&format!("Workspace name: {}", workspace_name))?;
                    logger.log(&format!("Region: {}", region))?;
                    
                    let spinner = output::create_spinner("Creating Coder workspace (1-2 minutes)...");
                    let config = CoderWorkspaceConfig {
                        template: "devdock-glean".to_string(),
                        workspace_name: workspace_name.clone(),
                        region: region.to_string(),
                        port_base: Some(offset),
                        profile: profile.to_string(),
                    };
                    
                    let ws_name = workspace::provision_workspace(&config).await?;
                    spinner.finish_and_clear();
                    output::success("Workspace created");
                    logger.log("Workspace created successfully")?;
                    logger.log(&format!("Workspace SSH ready: coder ssh {}", ws_name))?;
                    ws_name
                },
                "3" | "" => {
                    output::info("Cancelled");
                    return Err(anyhow!("User cancelled workspace setup"));
                },
                _ => {
                    output::error("Invalid choice. Cancelled.");
                    return Err(anyhow!("Invalid choice - setup cancelled"));
                }
            }
        } else {
            // Workspace doesn't exist, create it
            logger.log_section("Creating Workspace")?;
            logger.log("Template: devdock-glean")?;
            logger.log(&format!("Workspace name: {}", workspace_name))?;
            logger.log(&format!("Region: {}", region))?;
            
            // Create workspace with spinner
            let spinner = output::create_spinner("Creating Coder workspace (1-2 minutes)...");
            let config = CoderWorkspaceConfig {
                template: "devdock-glean".to_string(),
                workspace_name: workspace_name.clone(),
                region: region.to_string(),
                port_base: Some(offset),
                profile: profile.to_string(),
            };
            
            let ws_name = workspace::provision_workspace(&config).await?;
            spinner.finish_and_clear();
            output::success("Workspace created");
            logger.log("Workspace created successfully")?;
            logger.log(&format!("Workspace SSH ready: coder ssh {}", ws_name))?;
            ws_name
        }
    };
    
    // Start DevDock on remote with progress reporting
    println!();
    logger.log_section("Syncing Configuration")?;
    logger.log("Syncing profile, custom.ini, and gcloud credentials...")?;
    
    logger.log_section("Starting DevDock")?;
    logger.log(&format!("Profile: {}", profile))?;
    logger.log(&format!("Services filter: {:?}", services))?;
    logger.log(&format!("Command: devdock start {} {}", profile, services.as_ref().map(|s| format!("-s {}", s)).unwrap_or_default()))?;
    logger.log(&format!("Remote logs: coder ssh {} -- tail -f /tmp/devdock-start.log", &workspace_name))?;
    
    // Step 1: Config sync (timing handled internally by workspace functions)
    progress.start_step("⚙️", "Config");
    
    if verbose {
        println!();  // Newline for substeps
        output::info("Syncing configuration files to remote...");
    }
    
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
    )
    .await?;
    
    if verbose {
        output::success("DevDock started successfully");
    }
    logger.log("DevDock started successfully")?;
    
    // Note: We'll finish the progress steps after we know the duration
    // For now, assume config+devdock took most of the time
    
    // Save port mappings to database if auto-forwarding
    if let Some(mgr) = &port_mgr {
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
    }
    
    // Clean up stale sync files from old instances first
    cleanup_stale_sync_files(&state).await?;
    
    // Auto-start file sync (always enabled)
    if !verbose {
        // Show as compact progress step
        progress.start_step("🔄", "File sync");
    } else {
        println!();
    }
    
    let spinner = if verbose {
        Some(output::create_spinner("Starting file sync..."))
    } else {
        None
    };
    
    let repo_path = PathBuf::from(&worktree);
    let sync_result = match sync_daemon::start_daemon(&instance_id, &workspace_name, &repo_path, false) {
        Ok(_) => {
            state.save_sync_session(&instance_id, &workspace_name, None, false, "running").await?;
            if let Some(s) = spinner {
                s.finish_and_clear();
            }
            
            if verbose {
                output::success("File sync started");
            } else {
                progress.finish_step(0);
            }
            Ok(())
        }
        Err(e) => {
            warn!("Failed to start file sync: {}", e);
            if let Some(s) = spinner {
                s.finish_and_clear();
            }
            
            if verbose {
                output::warn("File sync failed (start manually: devdock sync start)");
            } else {
                println!("{}", " ⚠️  (start manually)".yellow());
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
    logger.log(&format!("Total time: {}", total_time))?;
    logger.log(&format!("Workspace: {}", workspace_name))?;
    logger.log(&format!("Tilt UI: http://localhost:{}", offset + 10350))?;
    
    // Print success message (clean and actionable)
    println!();
    
    if verbose {
        output::header("🚀 Remote DevDock is Ready!");
        println!("{}", format!("Total time: {}", total_time).green().bold());
        println!();
        println!("Access your remote DevDock:");
        println!("  {}: {}", "Workspace".cyan(), workspace_name);
        println!("  {}: coder ssh {}", "SSH".cyan(), workspace_name);
        println!("  {}: {}", "Tilt UI".cyan(), format!("http://localhost:{}", offset + 10350).cyan());
        println!();
        
        if auto_forward {
            println!("  {}: {}", "Port forwarding".cyan(), "enabled".green());
            println!("  View port mappings: {}", "devdock ports list".cyan());
        } else {
            println!("  💡 Tip: Forward ports with: {}", "devdock ports forward --all".yellow());
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
        println!("{}", format!("│  ✅ Ready in {}                      │", total_time).green());
        println!("{}", "╰────────────────────────────────────────╯".green());
        println!();
        println!("{} {}", "🌐 Tilt UI:".cyan(), format!("http://localhost:{}", offset + 10350).cyan().underline());
        println!("{} coder ssh {}", "📁 SSH:    ".dimmed(), workspace_name.dimmed());
        println!();
        
        // Show actionable next steps based on context
        println!("{}", "💡 Next steps:".bold());
        
        if !auto_forward {
            println!("   {} {}  {}", "•".cyan(), "devdock ports forward --all".cyan(), "(forward service ports)".dimmed());
        } else {
            println!("   {} {}  {}", "•".cyan(), "devdock ports list".cyan(), "(view port mappings)".dimmed());
        }
        
        println!("   {} {}  {}", "•".cyan(), "devdock status".cyan(), "(check service health)".dimmed());
        println!("   {} {}  {}", "•".cyan(), format!("open http://localhost:{}", offset + 10350).cyan(), "(view dashboard)".dimmed());
        println!();
    }
    
    // Send desktop notification
    output::notify(
        "DevDock Remote Ready! 🚀",
        &format!("Workspace '{}' ready in {}. Tilt UI at localhost:{}", 
                 workspace_name, total_time, offset + 10350)
    );
    
        Ok::<(), anyhow::Error>(())
    }.await;
    
    // Handle result and update status accordingly
    match setup_result {
        Ok(_) => {
            // Success - status already updated to Running above
            Ok(())
        }
        Err(e) => {
            // Failed - update status to Stopped
            info!("Setup failed, updating instance status to Stopped");
            if let Err(status_err) = state.update_status(&instance_id, InstanceStatus::Stopped).await {
                warn!("Failed to update instance status: {}", status_err);
            }
            
            logger.log_section("Setup Failed")?;
            logger.log(&format!("Error: {}", e))?;
            logger.log(&format!("Instance {} marked as Stopped", &instance_id[..8]))?;
            
            output::error(&format!("DevDock setup failed: {}", e));
            println!();
            println!("To clean up: devdock stop -i {}", &instance_id[..8]);
            Err(e)
        }
    }
}

/// Get current git worktree path.
fn get_current_worktree() -> Result<String> {
    use std::process::Command;
    
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()?;
    
    if !output.status.success() {
        return Err(anyhow!("Not in a git repository"));
    }
    
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Get current git branch name.
fn get_current_branch() -> Result<String> {
    use std::process::Command;
    
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()?;
    
    if !output.status.success() {
        return Err(anyhow!("Failed to get branch name"));
    }
    
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Sanitize workspace name for use with Coder.
/// Replaces non-alphanumeric characters with hyphens and truncates to 32 chars.
fn sanitize_workspace_name(name: &str) -> String {
    // Replace non-alphanumeric (except hyphens) with hyphens
    let sanitized: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    
    // Truncate to 32 characters (Coder workspace name limit)
    sanitized.chars().take(32).collect()
}

/// Find profile file path.
fn find_profile(profile: &str) -> Result<PathBuf> {
    let devdock_root = std::env::var("DEVDOCK_ROOT")
        .or_else(|_| std::env::var("PWD"))
        .unwrap_or_else(|_| ".".to_string());
    
    let base = PathBuf::from(devdock_root).join("devdock/profiles");
    
    // Try user_profiles first
    let user_profile = base.join("user_profiles").join(format!("{}.yaml", profile));
    if user_profile.exists() {
        return Ok(user_profile);
    }
    
    // Try integration_tests
    let test_profile = base.join("integration_tests").join(format!("{}.yaml", profile));
    if test_profile.exists() {
        return Ok(test_profile);
    }
    
    // Try base profiles
    let base_profile = base.join(format!("{}.yaml", profile));
    if base_profile.exists() {
        return Ok(base_profile);
    }
    
    Err(anyhow!("Profile not found: {}", profile))
}

/// Calculate port offset for new instance.
async fn calculate_port_offset(state: &StateManager, port_base: Option<u16>) -> Result<u16> {
    if let Some(base) = port_base {
        return Ok(base);
    }
    
    // Auto-allocate port offset - only consider active instances
    let instances = state.list_instances().await?;
    let used_ports: Vec<u16> = instances
        .iter()
        .filter(|i| matches!(i.status, InstanceStatus::Running | InstanceStatus::Starting))
        .map(|i| i.port_base)
        .collect();
    
    // Find first available offset in increments of 1000
    for offset in (2000..65000).step_by(1000) {
        if !used_ports.contains(&offset) {
            return Ok(offset);
        }
    }
    
    Err(anyhow!("No available port offsets"))
}

/// Get current timestamp.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
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
fn extract_devdock_id_from_filename(filename: &str) -> Option<String> {
    // Remove extensions and suffixes
    let base = filename
        .trim_end_matches(".pid")
        .trim_end_matches(".excludes")
        .trim_end_matches(".hb")
        .trim_end_matches(".stats")
        .trim_end_matches(".log")
        .trim_end_matches("_portforward");
    
    // UUID format: 8-4-4-4-12 (36 chars with 4 dashes)
    if base.len() == 36 && base.matches('-').count() == 4 {
        Some(base.to_string())
    } else {
        None
    }
}

