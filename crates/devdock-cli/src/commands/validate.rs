//! Profile validation command.

use crate::output;
use anyhow::{Context, Result};
use devdock_config::{DependencyResolver, ProfileLoader, ProfileValidator, ServiceConfig};
use std::path::{Path, PathBuf};

pub async fn execute(profile_name: &str, strict: bool) -> Result<()> {
    output::info(&format!("Validating profile: {profile_name}"));

    // Load profile
    let loader = ProfileLoader::new();
    let profile = loader.load(profile_name)?;

    // Validate profile
    let validator = ProfileValidator::new();
    validator.validate(&profile)?;
    validator.validate_gcloud_project(&profile)?;

    // Resolve dependencies
    let resolver = DependencyResolver::new();
    let resolved = resolver.resolve(&profile)?;

    // Print results
    output::success(&format!("Profile: {profile_name}"));
    println!("  Project: {}", profile.project);
    println!("  Platform: {:?}", profile.platform);
    println!("  Services: {}", profile.services.len());
    println!("  Infrastructure: {}", profile.infra.len());

    if !profile.pipelines.is_empty() {
        println!("  Pipelines: {}", profile.pipelines.len());
    }

    println!("\n  Resolved {} components:", resolved.len());
    for (i, component) in resolved.iter().enumerate() {
        println!("    {}. {}", i + 1, component);
    }

    if strict {
        let workspace_root = find_workspace_root()?;
        validate_service_configs(&profile, &resolved, &workspace_root)?;
    }

    output::success("All validations passed");
    Ok(())
}

fn find_workspace_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let mut dir = cwd.as_path();
    loop {
        if dir.join("devdock").join("config").exists() {
            return Ok(dir.to_path_buf());
        }
        dir = dir.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "Could not find workspace root (no devdock/config/ found in any parent directory)"
            )
        })?;
    }
}

fn validate_service_configs(
    profile: &devdock_core::profile::Profile,
    resolved: &[String],
    workspace_root: &Path,
) -> Result<()> {
    println!("\n  Validating service configs (--strict):");
    let mut errors = Vec::new();

    for component in resolved {
        let platform = std::env::var("CLOUD_PLATFORM").unwrap_or_default().to_lowercase();
        let config_paths = [
            workspace_root.join(format!(
                "devdock/config/services/{component}/devdock_config_{platform}.json"
            )),
            workspace_root.join(format!("devdock/config/services/{component}/devdock_config.json")),
        ];

        let mut found = false;
        for config_path in &config_paths {
            if config_path.exists() {
                found = true;
                match validate_single_config(config_path) {
                    Ok(()) => {
                        let is_explicit = profile.services.contains_key(component)
                            || profile.infra.contains_key(component);
                        let marker = if is_explicit { "explicit" } else { "transitive" };
                        println!("    [ok] {component} ({marker})");
                    }
                    Err(e) => {
                        errors.push(format!("{component}: {e:#}"));
                        println!("    [FAIL] {component}: {e:#}");
                    }
                }
                break;
            }
        }
        if !found {
            let infra_config_path = workspace_root
                .join(format!("devdock/config/infra/{component}/devdock_config.json"));
            if infra_config_path.exists() {
                match validate_single_config(&infra_config_path) {
                    Ok(()) => {
                        let is_explicit = profile.infra.contains_key(component);
                        let marker = if is_explicit { "explicit" } else { "transitive" };
                        println!("    [ok] {component} (infra, {marker})");
                    }
                    Err(e) => {
                        errors.push(format!("{component}: {e:#}"));
                        println!("    [FAIL] {component}: {e:#}");
                    }
                }
            } else if !workspace_root.join(format!("devdock/config/infra/{component}")).exists() {
                println!("    [skip] {component} (no devdock_config.json)");
            }
        }
    }

    if !errors.is_empty() {
        anyhow::bail!(
            "Failed to parse devdock_config.json for {} service(s):\n  - {}",
            errors.len(),
            errors.join("\n  - ")
        );
    }

    Ok(())
}

fn validate_single_config(config_path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(config_path)
        .context(format!("Failed to read {}", config_path.display()))?;
    let _config: ServiceConfig = serde_json::from_str(&content)
        .context(format!("Failed to parse {}", config_path.display()))?;
    Ok(())
}
