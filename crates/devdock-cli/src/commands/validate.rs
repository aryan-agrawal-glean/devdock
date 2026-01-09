//! Profile validation command.

use crate::output;
use anyhow::Result;
use devdock_config::{DependencyResolver, ProfileLoader, ProfileValidator};

pub async fn execute(profile_name: &str) -> Result<()> {
    output::info(&format!("Validating profile: {}", profile_name));

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
    output::success(&format!("Profile: {}", profile_name));
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

    output::success("All validations passed");
    Ok(())
}
