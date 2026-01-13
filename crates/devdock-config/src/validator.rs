//! Profile validation.

use devdock_core::{profile::Profile, Error, Result};
use std::process::Command;

/// Validates DevDock profiles.
pub struct ProfileValidator;

impl ProfileValidator {
    /// Create a new validator.
    pub fn new() -> Self {
        Self
    }

    /// Validate a profile (synchronous checks only).
    pub fn validate(&self, profile: &Profile) -> Result<()> {
        // Check project is not empty
        if profile.project.is_empty() {
            return Err(Error::InvalidProfile("Project name cannot be empty".to_string()));
        }

        // Validate no empty component names
        for name in profile.all_components() {
            if name.is_empty() {
                return Err(Error::InvalidProfile("Component name cannot be empty".to_string()));
            }
        }

        // Check for duplicate component names across categories
        let mut seen = std::collections::HashSet::new();
        for name in profile.all_components() {
            if !seen.insert(name.clone()) {
                return Err(Error::InvalidProfile(format!("Duplicate component name: {name}")));
            }
        }

        Ok(())
    }

    /// Validate that the current gcloud project matches the profile's project.
    /// Only validates for Google platform - skips for AWS and Azure.
    pub fn validate_gcloud_project(&self, profile: &Profile) -> Result<()> {
        use devdock_core::types::Platform;

        // Skip gcloud validation for non-Google platforms
        if !matches!(profile.platform, Platform::Google) {
            return Ok(());
        }

        let output = Command::new("gcloud")
            .args(["config", "get-value", "project"])
            .output()
            .map_err(|e| Error::InvalidProfile(format!("Failed to run gcloud: {e}")))?;

        if !output.status.success() {
            return Err(Error::InvalidProfile(
                "Failed to get current gcloud project. Is gcloud configured?".to_string(),
            ));
        }

        let current_project = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if current_project.is_empty() {
            return Err(Error::InvalidProfile(
                "No gcloud project is currently set. Run: gcloud config set project <project>"
                    .to_string(),
            ));
        }

        if current_project != profile.project {
            return Err(Error::InvalidProfile(format!(
                "GCloud project mismatch!\n\
                 Current gcloud project: {}\n\
                 Profile project: {}\n\n\
                 Please switch to the correct project:\n\
                 gcloud config set project {}",
                current_project, profile.project, profile.project
            )));
        }

        Ok(())
    }

    /// Validate a profile and return detailed errors.
    pub fn validate_detailed(&self, profile: &Profile) -> Vec<String> {
        let mut errors = Vec::new();

        if profile.project.is_empty() {
            errors.push("Project name cannot be empty".to_string());
        }

        for name in profile.all_components() {
            if name.is_empty() {
                errors.push("Found component with empty name".to_string());
            }
        }

        // Check for duplicates
        let mut seen = std::collections::HashSet::new();
        for name in profile.all_components() {
            if !seen.insert(name.clone()) {
                errors.push(format!("Duplicate component name: {name}"));
            }
        }

        errors
    }
}

impl Default for ProfileValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devdock_core::profile::{InfraConfig, ServiceConfig};
    use devdock_core::types::{DeploymentType, Platform};
    use std::collections::HashMap;

    #[test]
    fn test_valid_profile() {
        let mut infra = HashMap::new();
        infra.insert(
            "sql".to_string(),
            InfraConfig { deployment_type: DeploymentType::Local, config: HashMap::new() },
        );

        let mut services = HashMap::new();
        services.insert(
            "qe".to_string(),
            ServiceConfig {
                deployment_type: DeploymentType::Local,
                env: HashMap::new(),
                hot_reload: None,
                ini_overrides: Vec::new(),
                script: None,
                service: None,
                depends_on_services: None,
                timeout: None,
                config: HashMap::new(),
            },
        );

        let profile = Profile {
            project: "test-project".to_string(),
            platform: Platform::Google,
            infra,
            services,
            pipelines: HashMap::new(),
            test_suite: HashMap::new(),
            env: HashMap::new(),
            ini_overrides: Vec::new(),
            multi_project: None,
        };

        let validator = ProfileValidator::new();
        assert!(validator.validate(&profile).is_ok());
    }

    #[test]
    fn test_empty_project() {
        let profile = Profile {
            project: String::new(),
            platform: Platform::Google,
            infra: HashMap::new(),
            services: HashMap::new(),
            pipelines: HashMap::new(),
            test_suite: HashMap::new(),
            env: HashMap::new(),
            ini_overrides: Vec::new(),
            multi_project: None,
        };

        let validator = ProfileValidator::new();
        assert!(validator.validate(&profile).is_err());
    }
}
