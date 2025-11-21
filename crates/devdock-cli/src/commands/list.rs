//! List command implementation.

use crate::output;
use anyhow::Result;

pub async fn execute(_running: bool) -> Result<()> {
    output::info("Listing DevDock instances...");
    output::warn("List command not yet implemented - coming soon!");
    Ok(())
}

