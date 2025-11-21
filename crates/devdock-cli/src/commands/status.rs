//! Status command implementation.

use crate::output;
use anyhow::Result;

pub async fn execute(_detailed: bool) -> Result<()> {
    output::info("Checking DevDock status...");
    output::warn("Status command not yet implemented - coming soon!");
    Ok(())
}

