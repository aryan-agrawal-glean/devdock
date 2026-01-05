//! Remote DevDock workflow implementation.

use anyhow::{anyhow, Result};
use colored::Colorize;
use devdock_remote::{coder, workspace, CoderWorkspaceConfig};
use devdock_state::{instance::{Instance, InstanceStatus}, StateManager};
use devdock_sync::daemon as sync_daemon;
use std::path::PathBuf;
use tracing::{info, warn};

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
    
    // Determine workspace name
    let workspace_name = workspace_name.or_else(|| vm.clone()).unwrap_or_else(|| branch.clone());
    
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
    
    println!("{}", "🚀 Starting remote DevDock...".cyan().bold());
    
    // Create or start workspace
    let workspace_name = if let Some(existing) = vm {
        println!("Using existing workspace: {}", existing.green());
        coder::start_workspace(&existing)?;
        existing
    } else {
        println!("Creating new workspace: {}", workspace_name.green());
        let config = CoderWorkspaceConfig {
            template: "devdock-glean".to_string(),
            workspace_name: workspace_name.clone(),
            region: region.to_string(),
            port_base: Some(offset),
            profile: profile.to_string(),
        };
        
        workspace::provision_workspace(&config).await?
    };
    
    // Start DevDock on remote with optional port forwarding
    println!("Starting DevDock on remote workspace...");
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
    println!("Starting file sync...");
    let repo_path = PathBuf::from(&worktree);
    match sync_daemon::start_daemon(&instance_id, &workspace_name, &repo_path, false) {
        Ok(_) => {
            state.save_sync_session(&instance_id, &workspace_name, None, false, "running").await?;
            println!("{}", "✅ File sync started".green());
        }
        Err(e) => {
            warn!("Failed to start file sync: {}", e);
            println!("{}", "⚠️  File sync failed to start (you can start it manually with: devdock sync start)".yellow());
        }
    }
    
    // Update instance status
    state.update_status(&instance_id, InstanceStatus::Running).await?;
    
    // Print success message
    println!("\n{}", "✅ Remote DevDock running!".green().bold());
    println!("   Tilt UI: {}", format!("http://localhost:{}", offset + 10350).cyan());
    
    if auto_forward {
        println!("   Port forwarding: {}", "enabled".green());
        println!("   View port mappings: {}", "devdock ports".cyan());
    } else {
        println!("   Tip: Use {} to auto-forward all service ports", "--auto-forward".yellow());
    }
    
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

