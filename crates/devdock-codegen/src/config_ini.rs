//! devdock.ini configuration file generation.

use devdock_core::{profile::Profile, Result};
use std::collections::HashMap;

/// Generates devdock.ini configuration file.
pub struct ConfigIniGenerator;

impl ConfigIniGenerator {
    pub fn new() -> Self {
        Self
    }

    /// Generate devdock.ini from a profile.
    pub fn generate(&self, profile: &Profile) -> Result<String> {
        let mut ini = String::new();

        // [general] section
        ini.push_str("[general]\n");
        ini.push_str(&format!("project = {}\n", profile.project));
        ini.push_str(&format!("platform = {:?}\n", profile.platform));
        ini.push_str("\n");

        // [services] section
        if !profile.services.is_empty() {
            ini.push_str("[services]\n");
            for (name, config) in &profile.services {
                ini.push_str(&format!("{} = {:?}\n", name, config.deployment_type));
            }
            ini.push_str("\n");
        }

        // [infrastructure] section
        if !profile.infra.is_empty() {
            ini.push_str("[infrastructure]\n");
            for (name, config) in &profile.infra {
                ini.push_str(&format!("{} = {:?}\n", name, config.deployment_type));
            }
            ini.push_str("\n");
        }

        Ok(ini)
    }
}

impl Default for ConfigIniGenerator {
    fn default() -> Self {
        Self::new()
    }
}

