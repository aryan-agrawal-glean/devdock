//! Workspace lifecycle management.

use crate::{coder, config_sync, port_forward::PortForwardManager, ssh, RemoteError, Result};
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;
use tracing::{debug, info};

/// Provision a new Coder workspace.
pub async fn provision_workspace(config: &coder::CoderWorkspaceConfig) -> Result<String> {
    info!("Provisioning workspace: {}", config.workspace_name);
    
    // Create workspace
    let workspace_name = coder::create_workspace(config)?;
    
    // Wait for SSH to be ready
    ssh::wait_for_ssh_ready(&workspace_name, 300)?; // 5 minute timeout
    
    Ok(workspace_name)
}

/// Start DevDock on remote workspace with optional port forwarding.
pub async fn start_remote_devdock(
    workspace: &str,
    profile: &str,
    profile_path: &Path,
    devdock_id: &str,
    services: Option<&str>,
    auto_forward: bool,
    offset: u16,
) -> Result<Option<PortForwardManager>> {
    info!("Starting DevDock on remote workspace: {}", workspace);
    
    // 1. Sync configuration files
    info!("Syncing configuration to remote...");
    config_sync::sync_all_config(workspace, profile_path, None)?;
    
    // 2. Start DevDock on remote using Rust binary
    let start_cmd = if let Some(svc) = services {
        format!(
            "cd ~/scio && devdock start {} -s {}",
            profile, svc
        )
    } else {
        format!(
            "cd ~/scio && devdock start {}",
            profile
        )
    };
    
    info!("Starting DevDock with command: {}", start_cmd);
    println!("  Starting DevDock on remote workspace...");
    
    // Execute in background (don't wait for completion)
    match ssh::ssh_exec(workspace, &format!("nohup {} > /tmp/devdock-start.log 2>&1 &", start_cmd)) {
        Ok(_) => println!("  ✓ DevDock start command executed"),
        Err(e) => {
            eprintln!("  ⚠ Warning: DevDock start command may have failed: {}", e);
            eprintln!("  Continuing anyway - check remote logs if issues persist");
        }
    }
    
    // Give DevDock time to start
    println!("  Waiting for DevDock to initialize (10 seconds)...");
    sleep(Duration::from_secs(10));
    
    // Check if DevDock process is running
    println!("  Verifying DevDock started...");
    match ssh::ssh_exec(workspace, "pgrep -f 'devdock.sh' || pgrep -f 'tilt'") {
        Ok(output) => {
            if !output.trim().is_empty() {
                println!("  ✓ DevDock process detected (PID: {})", output.trim());
            } else {
                eprintln!("  ⚠ Warning: No DevDock process found yet");
                eprintln!("  Check logs: coder ssh {} -- tail -f /tmp/devdock-start.log", workspace);
            }
        },
        Err(_) => {
            eprintln!("  ⚠ Warning: Could not verify DevDock process");
        }
    }
    
    // 3. Setup port forwarding
    let port_mgr = if auto_forward {
        info!("Setting up auto port forwarding...");
        let mut mgr = PortForwardManager::from_profile(
            profile_path,
            workspace,
            devdock_id,
            offset,
        )?;
        mgr.start_all_forwards()?;
        Some(mgr)
    } else {
        // Manual mode: only forward Tilt UI
        info!("Forwarding Tilt UI only (use --auto-forward for all services)");
        let tilt_port = offset + 10350;
        let _ = coder::port_forward_single(workspace, tilt_port, 10350)?;
        None
    };
    
    // 4. Wait for Tilt UI to be ready
    wait_for_tilt_ui(offset + 10350, 300)?;
    
    Ok(port_mgr)
}

/// Wait for Tilt UI to become accessible with progress updates.
pub fn wait_for_tilt_ui(port: u16, timeout_secs: u64) -> Result<()> {
    use std::time::Instant;
    
    info!("Waiting for Tilt UI at http://localhost:{}", port);
    println!("  Waiting for Tilt UI to become ready (this can take 3-5 minutes)...");
    println!("  Check logs on remote: coder ssh <workspace> and look at tmux or DevDock logs");
    
    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let mut last_update = 0u64;
    
    loop {
        let elapsed = start.elapsed().as_secs();
        
        // Print progress every 10 seconds
        if elapsed - last_update >= 10 {
            println!("  Still waiting... ({}s elapsed, timeout in {}s)", 
                     elapsed, timeout_secs - elapsed);
            last_update = elapsed;
        }
        
        // Try to connect to Tilt UI
        if let Ok(_) = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)) {
            info!("Tilt UI is ready");
            println!("  ✓ Tilt UI is ready!");
            return Ok(());
        }
        
        if start.elapsed() > timeout {
            eprintln!("\n  ❌ Timeout: Tilt UI not ready after {} seconds", timeout_secs);
            eprintln!("  Debug steps:");
            eprintln!("    1. SSH into workspace: coder ssh <workspace-name>");
            eprintln!("    2. Check if DevDock started: ps aux | grep devdock");
            eprintln!("    3. Check Tilt: ps aux | grep tilt");
            eprintln!("    4. View logs: journalctl --user -f");
            return Err(RemoteError::CommandFailed(format!(
                "Tilt UI not ready after {} seconds",
                timeout_secs
            )));
        }
        
        sleep(Duration::from_secs(5));
    }
}

