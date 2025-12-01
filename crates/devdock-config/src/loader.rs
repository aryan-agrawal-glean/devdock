//! Profile loading from filesystem.

use devdock_core::{profile::Profile, Error, Result};
use std::path::{Path, PathBuf};

/// Loads DevDock profiles from various locations.
/// 
/// Profile resolution follows Python devdock.sh behavior:
/// 1. user_profiles/ subdirectory (highest priority)
/// 2. integration_tests/ subdirectory
/// 3. Root profiles directory (lowest priority)
pub struct ProfileLoader {
    /// Base paths for profiles (in priority order).
    base_paths: Vec<PathBuf>,
}

impl ProfileLoader {
    /// Create a new profile loader with default search paths.
    pub fn new() -> Self {
        let mut base_paths = Vec::new();

        // Get DEVDOCK_ROOT or fall back to current directory
        let devdock_root = std::env::var("DEVDOCK_ROOT")
            .map(PathBuf::from)
            .or_else(|_| std::env::current_dir())
            .unwrap_or_default();

        // Primary: devdock/profiles in DEVDOCK_ROOT
        base_paths.push(devdock_root.join("devdock/profiles"));

        // Fallback: current directory profiles
        if let Ok(cwd) = std::env::current_dir() {
            if cwd != devdock_root {
                base_paths.push(cwd.join("devdock/profiles"));
                base_paths.push(cwd.join("profiles"));
            }
        }

        // Home directory profiles
        if let Ok(home) = std::env::var("HOME") {
            base_paths.push(PathBuf::from(home).join(".devdock/profiles"));
        }

        Self { base_paths }
    }

    /// Create a profile loader with custom search paths.
    pub fn with_paths(paths: Vec<PathBuf>) -> Self {
        Self {
            base_paths: paths,
        }
    }

    /// Add a search path.
    pub fn add_path(&mut self, path: PathBuf) {
        self.base_paths.push(path);
    }

    /// Load a profile by name.
    pub fn load(&self, name: &str) -> Result<Profile> {
        let profile_path = self.find_profile(name)?;
        self.load_from_path(&profile_path)
    }

    /// Load a profile by name and return the path as well.
    pub fn load_with_path(&self, name: &str) -> Result<(Profile, PathBuf)> {
        let profile_path = self.find_profile(name)?;
        let profile = self.load_from_path(&profile_path)?;
        Ok((profile, profile_path))
    }

    /// Load a profile from a specific path.
    pub fn load_from_path(&self, path: &Path) -> Result<Profile> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            Error::InvalidProfile(format!("Failed to read profile file {}: {}", path.display(), e))
        })?;

        let profile: Profile = serde_yaml::from_str(&content).map_err(|e| {
            Error::InvalidProfile(format!("Failed to parse profile {}: {}", path.display(), e))
        })?;

        Ok(profile)
    }

    /// Find a profile file by name.
    /// 
    /// Resolution priority (matches Python devdock.sh):
    /// 1. user_profiles/<name>.yaml (highest priority)
    /// 2. integration_tests/<name>.yaml
    /// 3. <name>.yaml (root profiles directory)
    fn find_profile(&self, name: &str) -> Result<PathBuf> {
        let filename_yaml = format!("{}.yaml", name);
        let filename_yml = format!("{}.yml", name);

        // Subdirectories to check in priority order
        let subdirs = ["user_profiles", "integration_tests", ""];

        for base_path in &self.base_paths {
            for subdir in &subdirs {
                let search_path = if subdir.is_empty() {
                    base_path.clone()
                } else {
                    base_path.join(subdir)
                };

                // Try .yaml extension
                let yaml_path = search_path.join(&filename_yaml);
                if yaml_path.exists() {
                    return Ok(yaml_path);
                }

                // Try .yml extension
                let yml_path = search_path.join(&filename_yml);
                if yml_path.exists() {
                    return Ok(yml_path);
                }
            }
        }

        Err(Error::ProfileNotFound(PathBuf::from(name)))
    }

    /// List all available profiles grouped by category.
    pub fn list_profiles(&self) -> Result<Vec<String>> {
        let mut profiles = Vec::new();

        for base_path in &self.base_paths {
            // Check subdirectories and root
            let subdirs = ["user_profiles", "integration_tests", ""];
            
            for subdir in &subdirs {
                let search_path = if subdir.is_empty() {
                    base_path.clone()
                } else {
                    base_path.join(subdir)
                };

                if !search_path.exists() {
                    continue;
                }

                for entry in walkdir::WalkDir::new(&search_path)
                    .max_depth(1)
                    .into_iter()
                    .filter_map(|e| e.ok())
                {
                    let path = entry.path();
                    if let Some(ext) = path.extension() {
                        if ext == "yaml" || ext == "yml" {
                            if let Some(stem) = path.file_stem() {
                                if let Some(name) = stem.to_str() {
                                    profiles.push(name.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        profiles.sort();
        profiles.dedup();
        Ok(profiles)
    }

    /// List profiles with their categories (user_profiles, integration_tests, standard).
    pub fn list_profiles_with_category(&self) -> Result<Vec<(String, String)>> {
        let mut profiles = Vec::new();

        for base_path in &self.base_paths {
            let categories = [
                ("user_profiles", "User profiles"),
                ("integration_tests", "Integration test profiles"),
                ("", "Standard profiles"),
            ];
            
            for (subdir, category) in &categories {
                let search_path = if subdir.is_empty() {
                    base_path.clone()
                } else {
                    base_path.join(subdir)
                };

                if !search_path.exists() {
                    continue;
                }

                for entry in walkdir::WalkDir::new(&search_path)
                    .max_depth(1)
                    .into_iter()
                    .filter_map(|e| e.ok())
                {
                    let path = entry.path();
                    if let Some(ext) = path.extension() {
                        if ext == "yaml" || ext == "yml" {
                            if let Some(stem) = path.file_stem() {
                                if let Some(name) = stem.to_str() {
                                    profiles.push((name.to_string(), category.to_string()));
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(profiles)
    }
}

impl Default for ProfileLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_load_profile() {
        let temp_dir = TempDir::new().unwrap();
        let profile_path = temp_dir.path().join("test_profile.yaml");

        let profile_content = r#"
project: test-project
platform: google
infra:
  sql:
    type: local
services:
  qe:
    type: local
"#;
        std::fs::write(&profile_path, profile_content).unwrap();

        let loader = ProfileLoader::with_paths(vec![temp_dir.path().to_path_buf()]);
        let profile = loader.load("test_profile").unwrap();

        assert_eq!(profile.project, "test-project");
        assert_eq!(profile.infra.len(), 1);
        assert_eq!(profile.services.len(), 1);
    }

    #[test]
    fn test_profile_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let loader = ProfileLoader::with_paths(vec![temp_dir.path().to_path_buf()]);

        let result = loader.load("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_list_profiles() {
        let temp_dir = TempDir::new().unwrap();

        std::fs::write(temp_dir.path().join("profile1.yaml"), "project: test1").unwrap();
        std::fs::write(temp_dir.path().join("profile2.yaml"), "project: test2").unwrap();

        let loader = ProfileLoader::with_paths(vec![temp_dir.path().to_path_buf()]);
        let profiles = loader.list_profiles().unwrap();

        assert_eq!(profiles.len(), 2);
        assert!(profiles.contains(&"profile1".to_string()));
        assert!(profiles.contains(&"profile2".to_string()));
    }
}

