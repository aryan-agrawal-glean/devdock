//! Plugin management commands.

use crate::output;
use anyhow::Result;

pub async fn list() -> Result<()> {
    output::warn("Plugin list not yet implemented - coming soon!");
    Ok(())
}

pub async fn install(_name: &str) -> Result<()> {
    output::warn("Plugin install not yet implemented - coming soon!");
    Ok(())
}

pub async fn remove(_name: &str) -> Result<()> {
    output::warn("Plugin remove not yet implemented - coming soon!");
    Ok(())
}

