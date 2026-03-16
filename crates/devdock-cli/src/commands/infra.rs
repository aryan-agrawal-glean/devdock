//! Infrastructure management commands.
//!
//! Provides `devdock infra <component> <action>` for managing infrastructure.
//! Each command delegates to the corresponding plugin (e.g., sql-hydrator,
//! opensearch-hydrator) via the plugin protocol.

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

/// Execute `devdock infra sql setup` — delegates to devdock-plugin-sql-hydrator.
#[allow(clippy::too_many_arguments)]
pub async fn sql_setup(
    project: &str,
    platform: &str,
    region: &str,
    db_tables: &str,
    mode: &str,
    full_cleanup: bool,
    copy_keyval: bool,
    use_full_instance: bool,
    use_mysql_seed: bool,
) -> Result<()> {
    let plugin_name = "devdock-plugin-sql-hydrator";

    let params = serde_json::json!({
        "project": project,
        "platform": platform,
        "region": region,
        "db_tables": db_tables,
        "mode": mode,
        "full_cleanup": full_cleanup,
        "copy_keyval": copy_keyval,
        "use_full_instance": use_full_instance,
        "use_mysql_seed": use_mysql_seed,
        "workspace_root": std::env::var("DEVDOCK_ROOT")
            .unwrap_or_else(|_| std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()),
    });

    run_plugin_command(plugin_name, "sql_setup", &params)
}

/// Execute `devdock infra opensearch copy` — delegates to devdock-plugin-opensearch-hydrator.
pub async fn opensearch_copy() -> Result<()> {
    let plugin_name = "devdock-plugin-opensearch-hydrator";

    let params = serde_json::json!({
        "workspace_root": std::env::var("DEVDOCK_ROOT")
            .unwrap_or_else(|_| std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()),
    });

    run_plugin_command(plugin_name, "opensearch_copy", &params)
}

/// Spawn a plugin process and send a single command via the plugin protocol.
fn run_plugin_command(plugin_name: &str, method: &str, params: &serde_json::Value) -> Result<()> {
    let plugin_path = resolve_plugin(plugin_name)?;

    let mut child = Command::new(&plugin_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("Failed to start plugin '{plugin_name}' at '{plugin_path}'"))?;

    let stdin = child.stdin.as_mut().context("Plugin stdin not available")?;
    let stdout = child.stdout.as_mut().context("Plugin stdout not available")?;
    let mut reader = BufReader::new(stdout);

    // Initialize
    let init_msg = serde_json::json!({"method": "initialize", "config": {}});
    writeln!(stdin, "{init_msg}")?;
    stdin.flush()?;
    let mut response = String::new();
    reader.read_line(&mut response)?;

    // Send command
    let cmd_msg = serde_json::json!({"method": method, "params": params});
    writeln!(stdin, "{cmd_msg}")?;
    stdin.flush()?;
    response.clear();
    reader.read_line(&mut response)?;

    // Check response
    let resp: serde_json::Value =
        serde_json::from_str(response.trim()).context("Failed to parse plugin response")?;

    if resp.get("status").and_then(|s| s.as_str()) == Some("error") {
        let msg = resp.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown error");
        anyhow::bail!("Plugin '{plugin_name}' error: {msg}");
    }

    // Shutdown
    let shutdown_msg = serde_json::json!({"method": "shutdown"});
    writeln!(stdin, "{shutdown_msg}")?;
    stdin.flush()?;
    let _ = child.wait();

    Ok(())
}

/// Resolve plugin executable path (same logic as PluginManager).
fn resolve_plugin(name: &str) -> Result<String> {
    // Check ~/.devdock/plugins/
    if let Ok(home) = std::env::var("HOME") {
        let plugin_path = std::path::PathBuf::from(home).join(".devdock/plugins").join(name);
        if plugin_path.exists() {
            return Ok(plugin_path.to_string_lossy().to_string());
        }
    }

    // Check $PATH
    if let Ok(path) = which::which(name) {
        return Ok(path.to_string_lossy().to_string());
    }

    anyhow::bail!(
        "Plugin '{name}' not found. Install it to ~/.devdock/plugins/ or ensure it's in $PATH."
    )
}
