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

/// Start remote DevDock workflow.
pub async fn start_remote(
    profile: &str,
    workspace_name: Option<String>,
    region: &str,
    port_base: Option<u16>,
    vm: Option<String>,
    auto_forward: bool,
    services: Option<&str>,
) -> Result<()> {
    info!("Starting remote DevDock workflow");
    
    // Get current git info
    let worktree = get_current_worktree()?;
    let branch = get_current_branch()?;
    
    // Determine workspace name and sanitize it (replace slashes and special chars)
    let workspace_name = workspace_name
        .or_else(|| vm.clone())
        .unwrap_or_else(|| branch.clone());
    let workspace_name = sanitize_workspace_name(&workspace_name);
    
    // Find profile path
    let profile_path = find_profile(profile)?;
    
    // Register instance in database
    let state = StateManager::new().await?;
    let instance_id = uuid::Uuid::new_v4().to_string();
    let offset = calculate_port_offset(&state, port_base).await?;
    
    let instance = Instance {
        id: instance_id.clone(),
        workspace_path: worktree.clone(),
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
    
    // Display initial info
    println!();
    output::header("Starting Remote DevDock");
    println!("  {}: {}", "Profile".cyan(), profile);
    println!("  {}: {}", "Branch".cyan(), branch);
    println!("  {}: {}", "Workspace".cyan(), workspace_name.green());
    println!("  {}: devdock-glean", "Template".cyan());
    println!("  {}: {}", "Region".cyan(), region);
    println!("  {}: {}", "DevDock ID".cyan(), &instance_id[..8]);
    println!("  {}: {}", "Port Base".cyan(), offset);
    println!();
    println!("{}", "⏱️  Expected time: ~8-10 minutes total".yellow());
    println!("   • Creating workspace: ~1-2 min");
    println!("   • Initializing (repo, venv): ~2-3 min");
    println!("   • Starting DevDock + Tilt: ~5 min");
    println!("   📬 You will receive a notification when ready!");
    println!();
    println!("📋 Session logs: {}", logger.path().display().to_string().cyan());
    println!("   Monitor progress: tail -f {}", logger.path().display());
    println!();
    
    // Create or start workspace
    let workspace_name = if let Some(existing) = vm {
        logger.log_section("Using Existing Workspace")?;
        logger.log(&format!("Workspace: {}", existing))?;
        output::info(&format!("Using existing workspace: {}", existing));
        coder::start_workspace(&existing)?;
        logger.log("Workspace started successfully")?;
        existing
    } else {
        logger.log_section("Creating Workspace")?;
        logger.log(&format!("Template: devdock-glean"))?;
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
    };
    
    // Start DevDock on remote
    println!();
    logger.log_section("Starting DevDock")?;
    logger.log(&format!("Profile: {}", profile))?;
    logger.log(&format!("Services filter: {:?}", services))?;
    logger.log("This may take 3-5 minutes...")?;
    
    output::info("Starting DevDock on remote (this may take 3-5 minutes)...");
    logger.log(&format!("Remote logs: coder ssh {} -- tail -f /tmp/devdock-start.log", &workspace_name))?;
    
    let port_mgr = workspace::start_remote_devdock(
        &workspace_name,
        profile,
        &profile_path,
        &instance_id,
        services,
        auto_forward,
        offset,
    )
    .await?;
    
    output::success("DevDock started successfully");
    logger.log("DevDock started successfully")?;
    
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
    
    // Auto-start file sync (always enabled)
    println!();
    let spinner = output::create_spinner("Starting file sync...");
    let repo_path = PathBuf::from(&worktree);
    match sync_daemon::start_daemon(&instance_id, &workspace_name, &repo_path, false) {
        Ok(_) => {
            state.save_sync_session(&instance_id, &workspace_name, None, false, "running").await?;
            spinner.finish_and_clear();
            output::success("File sync started");
        }
        Err(e) => {
            warn!("Failed to start file sync: {}", e);
            spinner.finish_and_clear();
            output::warn("File sync failed (start manually: devdock sync start)");
        }
    }
    
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
    
    // Print success message
    println!();
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
        println!("  View port mappings: {}", "devdock ports".cyan());
    } else {
        println!("  💡 Tip: Use {} to auto-forward all service ports", "--auto-forward".yellow());
    }
    
    println!();
    println!("Forward additional ports:");
    println!("  QE: coder port-forward {} --tcp 7960:7960", workspace_name);
    println!();
    
    // Send desktop notification
    output::notify(
        "DevDock Remote Ready! 🚀",
        &format!("Workspace '{}' ready in {}. Tilt UI at localhost:{}", 
                 workspace_name, total_time, offset + 10350)
    );
    
    Ok(())
}

/// Get current git worktree path.
fn get_current_worktree() -> Result<String> {
    use std::process::Command;
    
    let output = Command::new("git")
        .args(&["rev-parse", "--show-toplevel"])
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
        .args(&["rev-parse", "--abbrev-ref", "HEAD"])
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
    
    // Auto-allocate port offset
    let instances = state.list_instances().await?;
    let used_ports: Vec<u16> = instances.iter().map(|i| i.port_base).collect();
    
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

