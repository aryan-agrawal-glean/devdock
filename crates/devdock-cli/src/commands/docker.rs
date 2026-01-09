//! Docker management commands.

use crate::output;
use anyhow::Result;
use devdock_config::docker;

/// Check if Docker is running
pub async fn check() -> Result<()> {
    output::info("Checking Docker status...");
    docker::check_docker()?;
    output::success("✅ Docker is running and ready");
    Ok(())
}

/// Setup Docker networks
pub async fn networks() -> Result<()> {
    output::info("Setting up Docker networks...");
    docker::check_docker()?;
    docker::setup_networks()?;
    output::success("✅ Docker networks are ready");

    // List the networks
    for network in docker::DEVDOCK_NETWORKS {
        output::info(&format!("  • {}", network));
    }

    Ok(())
}

/// Pull Docker images
pub async fn pull(python_only: bool, cache: bool) -> Result<()> {
    output::info("Pulling Docker images...");
    docker::check_docker()?;

    // Login to GCR first
    output::info("Authenticating with Google Container Registry...");
    docker::docker_login_gcr()?;
    output::success("Authenticated with GCR");

    let images = if python_only {
        output::info("Pulling Python images only...");
        docker::PYTHON_IMAGES
    } else {
        output::info("Pulling all required images...");
        docker::REQUIRED_IMAGES
    };

    docker::pull_images(images, cache)?;

    output::success(&format!("✅ Pulled {} images", images.len()));

    if cache {
        output::info("Images have been cached to ~/.devdock/images/");
    }

    Ok(())
}

/// Login to Google Container Registry
pub async fn login() -> Result<()> {
    output::info("Logging into Google Container Registry...");
    docker::docker_login_gcr()?;
    output::success("✅ Successfully logged into GCR");
    Ok(())
}

/// Remove dangling images
pub async fn prune() -> Result<()> {
    output::info("Pruning dangling Docker images...");
    docker::check_docker()?;

    let removed = docker::remove_dangling_images()?;

    if removed > 0 {
        output::success(&format!("✅ Removed {} dangling images", removed));
    } else {
        output::info("No dangling images to remove");
    }

    Ok(())
}
