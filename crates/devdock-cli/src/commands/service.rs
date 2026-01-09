//! Service management commands.

use crate::output;
use anyhow::Result;

pub async fn enable(_name: &str) -> Result<()> {
    output::warn("Service enable not yet implemented - coming soon!");
    Ok(())
}

pub async fn disable(_name: &str) -> Result<()> {
    output::warn("Service disable not yet implemented - coming soon!");
    Ok(())
}

pub async fn logs(_name: &str, _follow: bool) -> Result<()> {
    output::warn("Service logs not yet implemented - coming soon!");
    Ok(())
}
