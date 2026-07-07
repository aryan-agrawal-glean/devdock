//! Service management commands.

use crate::output;
use anyhow::Result;

pub async fn enable(name: &str) -> Result<()> {
    output::warn(&format!(
        "Service enable is not implemented yet for '{name}'. For now, use `devdock profile apply --services <svc1,svc2,...>` or run `devdock guide add-service`."
    ));
    Ok(())
}

pub async fn disable(name: &str) -> Result<()> {
    output::warn(&format!(
        "Service disable is not implemented yet for '{name}'. For now, update the active service list with `devdock profile apply --services <svc1,svc2,...>` or run `devdock guide add-service`."
    ));
    Ok(())
}

pub async fn logs(name: &str, _follow: bool) -> Result<()> {
    output::warn(&format!(
        "Service logs is not implemented yet for '{name}'. For now, use Tilt/Docker logs directly or run `devdock guide troubleshoot`."
    ));
    Ok(())
}
