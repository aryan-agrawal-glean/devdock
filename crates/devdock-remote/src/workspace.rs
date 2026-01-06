//! Workspace lifecycle management.

use crate::{coder, config_sync, port_forward::PortForwardManager, ssh, RemoteError, Result};
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;
use tracing::info;

/// Cleanup existing DevDock session on remote workspace.
pub fn cleanup_existing_devdock(workspace: &str, log_file: Option<&Path>) -> Result<()> {
    info!("Cleaning up existing DevDock processes on {}", workspace);
    
    // Optimized cleanup: Single SSH session with parallelized independent operations
    // This reduces 5 sequential SSH calls (~10 seconds) to 1 batched call (~2 seconds)
    let cleanup_script = r#"
# Parallelize independent cleanup operations for speed
# Each operation runs in background, we wait for all to complete

# 1. Kill tmux session (background)
(tmux kill-session -t devdock 2>/dev/null || true) &
pid_tmux=$!

# 2. Kill tilt processes (background)
(pkill -f 'tilt up' 2>/dev/null || true) &
pid_tilt=$!

# 3. Clean up Docker containers - optimized bulk operation (background)
(
  # Use -q for just IDs, then bulk remove (faster than xargs with names)
  containers=$(docker ps -a --filter 'name=devdock-' -q 2>/dev/null)
  if [ -n "$containers" ]; then
    docker rm -f $containers 2>/dev/null || true
  fi
) &
pid_docker=$!

# 4. Clean up DevDock database and pid files (background)
(rm -f ~/.devdock/devdock.db ~/.devdock/*.pid 2>/dev/null || true) &
pid_files=$!

# Wait for all parallel operations to complete
wait $pid_tmux $pid_tilt $pid_docker $pid_files

# 5. Run comprehensive devdock cleanup (sequential, after others complete)
# This ensures all resources are cleaned before running the full cleanup
cd /home/workspace/scio && devdock cleanup 2>/dev/null || true

echo "Cleanup complete"
"#;
    
    match ssh::ssh_exec_with_log(workspace, cleanup_script, log_file) {
        Ok(_) => {
            info!("Cleanup completed successfully");
            Ok(())
        }
        Err(e) => {
            // Log error but don't fail - cleanup is best-effort
            info!("Cleanup encountered errors (non-fatal): {}", e);
            Ok(())
        }
    }
}

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
    log_file: Option<&Path>,
) -> Result<Option<PortForwardManager>> {
    info!("Starting DevDock on remote workspace: {}", workspace);
    
    // Log start
    if let Some(log_path) = log_file {
        use std::fs::OpenOptions;
        use std::io::Write;
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
            let _ = writeln!(file, "\n=== Starting DevDock on Remote Workspace ===");
            let _ = writeln!(file, "Workspace: {}", workspace);
            let _ = writeln!(file, "Profile: {}", profile);
            let _ = writeln!(file, "DevDock ID: {}", devdock_id);
            let _ = writeln!(file, "Port Offset: {}", offset);
            let _ = writeln!(file, "Services: {:?}", services);
        }
    }
    
    // 1. Sync configuration files
    info!("Syncing configuration to remote...");
    println!("  Syncing profile: {:?}", profile_path);
    config_sync::sync_all_config(workspace, profile_path, None, log_file)?;
    println!("  ✓ Configuration synced");
    
    // 2. Start DevDock in tmux session on remote
    // Export environment variables before tmux, they will be inherited by the session
    let start_script = if let Some(svc) = services {
        format!(
            "export DEVDOCK_ID={} && export DEVDOCK_PORT_OFFSET={} && cd /home/workspace/scio && tmux new-session -d -s devdock 'devdock start {} -s {}'",
            devdock_id, offset, profile, svc
        )
    } else {
        format!(
            "export DEVDOCK_ID={} && export DEVDOCK_PORT_OFFSET={} && cd /home/workspace/scio && tmux new-session -d -s devdock 'devdock start {}'",
            devdock_id, offset, profile
        )
    };
    
    info!("Starting DevDock in tmux session");
    println!("  Starting DevDock on remote workspace...");
    println!("  Profile: {}", profile);
    println!("  DevDock ID: {}", &devdock_id[..8.min(devdock_id.len())]);
    println!("  Port Offset: {}", offset);
    
    match ssh::ssh_exec_with_log(workspace, &start_script, log_file) {
        Ok(output) => {
            println!("  ✓ DevDock start command executed");
            // Don't print environment variable dumps
            if !output.trim().is_empty() && !output.trim().contains("tmux") && !output.trim().contains("declare -x") {
                println!("  Output: {}", output.trim());
            }
        },
        Err(e) => {
            let err_msg = format!("{}", e);
            // Check for duplicate session error
            if err_msg.contains("duplicate session") {
                println!("  ⚠ Duplicate tmux session detected, cleaning up...");
                
                // Kill existing session
                let _ = ssh::ssh_exec_with_log(workspace, "tmux kill-session -t devdock 2>/dev/null || true", log_file);
                sleep(Duration::from_secs(2));
                
                // Retry
                println!("  Retrying DevDock start...");
                match ssh::ssh_exec_with_log(workspace, &start_script, log_file) {
                    Ok(_) => println!("  ✓ Retry succeeded"),
                    Err(e2) => {
                        eprintln!("  ❌ ERROR: Retry failed: {}", e2);
                        return Err(e2);
                    }
                }
            } else {
                eprintln!("  ❌ ERROR: DevDock start command failed: {}", e);
                return Err(e);
            }
        }
    }
    
    println!("  ✓ DevDock command started in background");
    println!("  To view logs: coder ssh {} -- tmux attach -t devdock", workspace);
    
    // 3. Wait for Tilt process to start (check if port 10350 is in use)
    println!("  Waiting for Tilt to start (checking port 10350)...");
    let timeout = Duration::from_secs(30);
    let start = std::time::Instant::now();
    
    loop {
        let elapsed = start.elapsed().as_secs();
        
        // Check if Tilt is listening on port 10350 using lsof
        match ssh::ssh_check(workspace, "lsof -i:10350 >/dev/null 2>&1", log_file) {
            Ok(true) => {
                println!("  ✓ Tilt process is running on port 10350!");
                break;
            },
            Ok(false) => {
                // Not running yet, continue waiting
            },
            Err(_) => {
                // Error executing command, continue waiting
            }
        }
        
        // Print progress every 5 seconds
        if elapsed > 0 && elapsed % 5 == 0 {
            println!("  Still waiting for Tilt... ({}s elapsed)", elapsed);
        }
        
        if start.elapsed() > timeout {
            eprintln!("  ⚠ Warning: Tilt not detected on port 10350 after 30 seconds");
            eprintln!("  This may be normal if services are still starting up");
            eprintln!("  Continuing to wait for Tilt UI to be ready...");
            break;
        }
        
        sleep(Duration::from_secs(2));
    }
    
    println!("\n  To view DevDock logs:");
    println!("    coder ssh {}", workspace);
    println!("    tmux attach -t devdock");
    
    // 4. Setup port forwarding
    println!("\n  Setting up port forwarding...");
    let port_mgr = if auto_forward {
        info!("Setting up auto port forwarding...");
        let mut mgr = PortForwardManager::from_profile(
            profile_path,
            workspace,
            devdock_id,
            offset,
        )?;
        mgr.start_all_forwards()?;
        println!("  ✓ Auto port forwarding enabled");
        Some(mgr)
    } else {
        // Manual mode: only forward Tilt UI
        info!("Forwarding Tilt UI only (use --auto-forward for all services)");
        let tilt_port = offset + 10350;
        println!("  Starting port forward: localhost:{} -> remote:10350", tilt_port);
        let _child = coder::port_forward_single(workspace, tilt_port, 10350)?;
        // Keep the child process alive - it will be cleaned up when the program exits
        std::mem::forget(_child);
        println!("  ✓ Tilt UI port forward started");
        None
    };
    
    // 5. Final success message
    println!("\n  ✅ DevDock is running!");
    println!("  Tilt UI: http://localhost:{}", offset + 10350);
    println!("  Logs: coder ssh {} && tmux attach -t devdock", workspace);
    
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

