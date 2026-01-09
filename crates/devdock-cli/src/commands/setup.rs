//! Setup command implementation.

use crate::output;
use anyhow::{Context, Result};
use devdock_config::docker;
use std::path::PathBuf;
use std::process::Command;

/// Execute the setup command
pub async fn execute(remote: bool) -> Result<()> {
    if remote {
        setup_remote().await
    } else {
        setup_local().await
    }
}

/// Setup local DevDock environment
async fn setup_local() -> Result<()> {
    output::header("DevDock Local Setup");

    // Step 1: Check Docker
    output::step(1, 6, "Checking Docker...");
    docker::check_docker()?;
    output::success("Docker is running");

    // Step 2: Setup Docker networks
    output::step(2, 6, "Setting up Docker networks...");
    docker::setup_networks()?;
    output::success("Networks created");

    // Step 3: Create directories
    output::step(3, 6, "Creating directories...");
    let home = std::env::var("HOME")?;
    let devdock_dir = PathBuf::from(&home).join(".devdock");

    let dirs = [
        devdock_dir.join("sql"),
        devdock_dir.join("sql/scripts"),
        devdock_dir.join("opensearch"),
        devdock_dir.join("images"),
        devdock_dir.join("db_manager"),
    ];

    for dir in &dirs {
        std::fs::create_dir_all(dir)?;
    }
    output::success("Directories created");

    // Step 4: Check gcloud
    output::step(4, 6, "Checking gcloud CLI...");
    let gcloud_check = Command::new("gcloud").args(["--version"]).output();

    if gcloud_check.is_err() || !gcloud_check.unwrap().status.success() {
        output::warn("gcloud CLI not found. Please install Google Cloud SDK.");
        output::info("Visit: https://cloud.google.com/sdk/docs/install");
    } else {
        output::success("gcloud CLI available");
    }

    // Step 5: Check Tilt
    output::step(5, 6, "Checking Tilt...");
    let tilt_check = Command::new("tilt").args(["version"]).output();

    if tilt_check.is_err() || !tilt_check.unwrap().status.success() {
        output::warn("Tilt not found. Please install Tilt.");
        output::info("Visit: https://docs.tilt.dev/install.html");
    } else {
        output::success("Tilt available");
    }

    // Step 6: Pull base images
    output::step(6, 6, "Would you like to pull Docker images now?");
    output::info("This can take several minutes but speeds up first run.");
    output::info("You can skip this and run 'devdock docker pull' later.");

    // In non-interactive mode, skip the pull
    output::info("Run 'devdock docker pull' to download required images.");

    output::header("Setup Complete");
    output::success("✅ DevDock local setup complete!");
    println!();
    output::info("Next steps:");
    output::info("  1. Run 'devdock docker pull' to download Docker images");
    output::info("  2. Run 'devdock start <profile>' to start DevDock");
    output::info("  3. Visit http://localhost:10350 for the Tilt UI");

    Ok(())
}

/// Setup remote DevDock environment (Coder workspaces)
async fn setup_remote() -> Result<()> {
    output::header("DevDock Remote Setup");

    // Step 1: Check Coder CLI
    output::step(1, 4, "Checking Coder CLI...");
    let coder_check = Command::new("coder").args(["version"]).output();

    if coder_check.is_err() || !coder_check.unwrap().status.success() {
        output::warn("Coder CLI not found.");
        output::info("Installing Coder CLI...");

        // Try to install coder
        let install = Command::new("sh")
            .args(["-c", "curl -fsSL https://coder.com/install.sh | sh"])
            .status();

        if install.is_err() || !install.unwrap().success() {
            anyhow::bail!(
                "Failed to install Coder CLI.\n\
                Please install manually: https://coder.com/docs/coder-oss/latest/install"
            );
        }
        output::success("Coder CLI installed");
    } else {
        output::success("Coder CLI available");
    }

    // Step 2: Check Coder login
    output::step(2, 4, "Checking Coder authentication...");
    let coder_whoami = Command::new("coder").args(["whoami"]).output();

    if coder_whoami.is_err() || !coder_whoami.unwrap().status.success() {
        output::warn("Not logged into Coder.");
        output::info("Please run: coder login <coder-url>");
        output::info("Then re-run this setup.");
    } else {
        output::success("Coder authenticated");
    }

    // Step 3: Check SSH config
    output::step(3, 4, "Setting up SSH config for Coder...");
    let ssh_config = Command::new("coder")
        .args(["config-ssh", "--yes"])
        .status()
        .context("Failed to configure SSH for Coder")?;

    if ssh_config.success() {
        output::success("SSH configured for Coder workspaces");
    } else {
        output::warn("SSH config may not be complete. Check manually.");
    }

    // Step 4: Create local directories
    output::step(4, 4, "Creating local directories...");
    let home = std::env::var("HOME")?;
    let devdock_dir = PathBuf::from(&home).join(".devdock");

    std::fs::create_dir_all(devdock_dir.join("sync"))?;
    std::fs::create_dir_all(devdock_dir.join("remote_logs"))?;
    output::success("Directories created");

    output::header("Remote Setup Complete");
    output::success("✅ DevDock remote setup complete!");
    println!();
    output::info("Next steps:");
    output::info("  1. Start a remote DevDock: devdock start <profile> --remote");
    output::info("  2. Or use slim mode: devdock start <profile> --remote --slim");
    output::info("  3. File sync will be started automatically in slim mode");

    Ok(())
}
