//! Exec into container command.

use crate::output;
use anyhow::Result;
use devdock_config::docker;

/// Execute into a container
pub async fn execute(container: &str, command: &str) -> Result<()> {
    // Check Docker is running
    docker::check_docker()?;

    output::info(&format!("Connecting to container: {}", container));

    // Execute the command
    docker::exec_container(container, command)?;

    Ok(())
}
