//! Stop command implementation.

use crate::output;
use anyhow::Result;

pub async fn execute(_target: Option<&str>, _force: bool) -> Result<()> {
    output::info("Stopping DevDock...");
    output::warn("Stop command not yet implemented - coming soon!");
    Ok(())
}

