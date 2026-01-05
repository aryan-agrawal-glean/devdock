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
    
    // 2. Start DevDock on remote
    let start_cmd = if let Some(svc) = services {
        format!(
            "cd ~/scio && source python_scio/scio_env/bin/activate && devdock/devdock.sh -p {} -s {}",
            profile, svc
        )
    } else {
        format!(
            "cd ~/scio && source python_scio/scio_env/bin/activate && devdock/devdock.sh -p {}",
            profile
        )
    };
    
    info!("Starting DevDock with command: {}", start_cmd);
    
    // Execute in background (don't wait for completion)
    let _ = ssh::ssh_exec(workspace, &format!("nohup {} > /dev/null 2>&1 &", start_cmd));
    
    // Give DevDock time to start
    sleep(Duration::from_secs(10));
    
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

/// Wait for Tilt UI to become accessible.
pub fn wait_for_tilt_ui(port: u16, timeout_secs: u64) -> Result<()> {
    use std::time::Instant;
    
    info!("Waiting for Tilt UI at http://localhost:{}", port);
    
    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    
    loop {
        // Try to connect to Tilt UI
        if let Ok(_) = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)) {
            info!("Tilt UI is ready");
            return Ok(());
        }
        
        if start.elapsed() > timeout {
            return Err(RemoteError::CommandFailed(format!(
                "Tilt UI not ready after {} seconds",
                timeout_secs
            )));
        }
        
        debug!("Tilt UI not ready yet, retrying...");
        sleep(Duration::from_secs(5));
    }
}

