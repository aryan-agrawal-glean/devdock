//! Profile management commands.

use crate::output;
use anyhow::Result;
use devdock_config::ProfileLoader;

pub async fn list() -> Result<()> {
    output::info("Available profiles:");

    let loader = ProfileLoader::new();
    let profiles = loader.list_profiles()?;

    if profiles.is_empty() {
        output::warn("No profiles found");
        return Ok(());
    }

    for profile in profiles {
        println!("  • {}", profile);
    }

    Ok(())
}

pub async fn show(name: &str) -> Result<()> {
    let loader = ProfileLoader::new();
    let profile = loader.load(name)?;

    output::header(&format!("Profile: {}", name));
    println!("Project: {}", profile.project);
    println!("Platform: {:?}", profile.platform);

    if !profile.infra.is_empty() {
        output::header("Infrastructure");
        for (name, config) in &profile.infra {
            println!("  • {} ({:?})", name, config.deployment_type);
        }
    }

    if !profile.services.is_empty() {
        output::header("Services");
        for (name, config) in &profile.services {
            println!("  • {} ({:?})", name, config.deployment_type);
        }
    }

    Ok(())
}

pub async fn create() -> Result<()> {
    output::warn("Profile create not yet implemented - coming soon!");
    Ok(())
}

