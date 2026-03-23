//! Init command — creates `.devdock/devdock.toml` for a new project.

use crate::output;
use anyhow::Result;
use std::path::Path;

pub async fn execute() -> Result<()> {
    let cwd = std::env::current_dir()?;

    if cwd.join(".devdock/devdock.toml").exists() {
        output::warn("DevDock already initialized (.devdock/devdock.toml exists)");
        return Ok(());
    }

    output::info("Initializing DevDock...");

    // Detect project name
    let project_name = detect_project_name(&cwd);

    // Detect features
    let has_bazel = cwd.join("WORKSPACE").exists() || cwd.join("WORKSPACE.bazel").exists();
    let has_services = cwd.join("devdock/config/services").exists();

    if has_bazel {
        output::info("Detected: Bazel workspace");
    }
    if has_services {
        output::info("Detected: Existing service configs");
    }

    // Create .devdock/devdock.toml
    std::fs::create_dir_all(cwd.join(".devdock"))?;
    std::fs::write(cwd.join(".devdock/devdock.toml"), generate_devdock_toml(&project_name))?;
    output::success("Created .devdock/devdock.toml");

    println!();
    output::success("DevDock initialized!");
    println!();
    println!("  Next steps:");
    println!("    1. Edit .devdock/devdock.toml with your project settings");
    println!("    2. Ensure devdock_config.json exists for each service");
    println!("    3. Run: devdock start <profile>");
    Ok(())
}

fn detect_project_name(cwd: &Path) -> String {
    if let Ok(output) = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(cwd)
        .output()
    {
        let url = String::from_utf8_lossy(&output.stdout);
        if let Some(name) = url.trim().rsplit('/').next() {
            let name = name.trim_end_matches(".git");
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    cwd.file_name().unwrap_or_default().to_string_lossy().to_string()
}

fn generate_devdock_toml(project_name: &str) -> String {
    format!(
        r#"[project]
name = "{project_name}"
domain = "localhost"
# services_dir = "devdock/config/services"
# infra_dir = "devdock/config/infra"

[registry]
# url = "ghcr.io/your-org"

[remote]
provider = "ssh"

[telemetry]
enabled = false

[cloud]
platform = "none"
# platform = "google"
# project = "your-gcp-project"
"#
    )
}
