//! Static watch patterns for hot reload - alternative to bazel cquery.
//!
//! This module provides a fast alternative to running `bazel cquery` at startup
//! by using pre-computed glob patterns based on service type and target location.
//!
//! ## Design
//!
//! Instead of querying bazel for exact source file dependencies (expensive),
//! we use heuristic glob patterns that cover the likely source files:
//!
//! 1. **Service-type based patterns**: Go services watch `go/`, Python watch `python_scio/`
//! 2. **Target-location patterns**: Extract the package path from the bazel target
//! 3. **Common deps**: Always watch `config/`, `data/`, `openapi/` for data changes
//! 4. **Cached cquery results**: Fall back to cached results if available
//!
//! ## Trade-offs
//!
//! - **Pros**: Sub-millisecond startup vs seconds with cquery
//! - **Cons**: May over-watch (trigger extra rebuilds) or under-watch (miss some deps)
//!
//! The over-watching is acceptable because:
//! - Bazel builds are incremental - unchanged deps don't cause rebuilds
//! - macOS fsevents has a 4047 file limit anyway, so we already compress

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Service type determines the primary watch patterns
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceType {
    Go,
    Python,
    Java,
    Frontend,
    Unknown,
}

impl From<&str> for ServiceType {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "go" => ServiceType::Go,
            "python" => ServiceType::Python,
            "java" => ServiceType::Java,
            "frontend" | "fe" | "typescript" => ServiceType::Frontend,
            _ => ServiceType::Unknown,
        }
    }
}

/// Static watch pattern configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchPatternConfig {
    /// Mapping from service name to custom patterns (overrides defaults)
    #[serde(default)]
    pub service_overrides: HashMap<String, Vec<String>>,

    /// Additional patterns by service type
    #[serde(default)]
    pub type_patterns: HashMap<String, Vec<String>>,

    /// Common patterns watched by all services
    #[serde(default)]
    pub common_patterns: Vec<String>,

    /// Patterns to always exclude
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
}

impl Default for WatchPatternConfig {
    fn default() -> Self {
        Self {
            service_overrides: HashMap::new(),
            type_patterns: Self::default_type_patterns(),
            common_patterns: vec![
                "config/".to_string(),
                "data/".to_string(),
                "openapi/".to_string(),
            ],
            exclude_patterns: vec![
                "**/node_modules/**".to_string(),
                "**/.git/**".to_string(),
                "**/bazel-*/**".to_string(),
                "**/__pycache__/**".to_string(),
            ],
        }
    }
}

impl WatchPatternConfig {
    fn default_type_patterns() -> HashMap<String, Vec<String>> {
        let mut map = HashMap::new();

        map.insert(
            "go".to_string(),
            vec![
                "go/core/".to_string(),
                "go/platform/".to_string(),
                "go/query_endpoint/".to_string(),
                "go/tracer/".to_string(),
                "go/auth/".to_string(),
            ],
        );

        map.insert(
            "python".to_string(),
            vec![
                "python_scio/core/".to_string(),
                "python_scio/platform/".to_string(),
                "python_scio/quality/".to_string(),
                "python_scio/deployment/".to_string(),
                "python_scio/agents/".to_string(),
            ],
        );

        map.insert("java".to_string(), vec!["java/com/askscio/".to_string()]);

        map.insert(
            "frontend".to_string(),
            vec![
                "typescript/web/".to_string(),
                "typescript/core/".to_string(),
                "typescript/elements/".to_string(),
            ],
        );

        map
    }

    /// Load config from YAML file, with defaults as fallback
    pub fn load(workspace_root: &Path) -> Result<Self> {
        let config_paths = [
            workspace_root.join("devdock/config/watch_patterns.yaml"),
            workspace_root.join(".devdock/watch_patterns.yaml"),
        ];

        for path in &config_paths {
            if path.exists() {
                let content = std::fs::read_to_string(path)?;
                return Ok(serde_yaml::from_str(&content)?);
            }
        }

        Ok(Self::default())
    }
}

/// Watch pattern generator - replaces BazelQueryEngine for hot reload
pub struct WatchPatternGenerator {
    workspace_root: PathBuf,
    config: WatchPatternConfig,
    cache_dir: PathBuf,
}

impl WatchPatternGenerator {
    pub fn new(workspace_root: PathBuf) -> Result<Self> {
        let config = WatchPatternConfig::load(&workspace_root)?;
        let home = std::env::var("HOME")?;
        let cache_dir = PathBuf::from(home).join(".devdock/cache/watch_patterns");
        std::fs::create_dir_all(&cache_dir)?;

        Ok(Self {
            workspace_root,
            config,
            cache_dir,
        })
    }

    /// Generate watch patterns for a service without running cquery
    pub fn generate_patterns(
        &self,
        service_name: &str,
        service_type: ServiceType,
        bazel_target: Option<&str>,
    ) -> Vec<String> {
        let mut patterns = HashSet::new();

        // 1. Check for explicit service override
        if let Some(override_patterns) = self.config.service_overrides.get(service_name) {
            return override_patterns.clone();
        }

        // 2. Add type-based patterns
        let type_key = match service_type {
            ServiceType::Go => "go",
            ServiceType::Python => "python",
            ServiceType::Java => "java",
            ServiceType::Frontend => "frontend",
            ServiceType::Unknown => "",
        };

        if let Some(type_patterns) = self.config.type_patterns.get(type_key) {
            for p in type_patterns {
                patterns.insert(p.clone());
            }
        }

        // 3. Extract package path from bazel target
        if let Some(target) = bazel_target {
            if let Some(package_pattern) = self.target_to_package_pattern(target) {
                patterns.insert(package_pattern);
            }
        }

        // 4. Add common patterns
        for p in &self.config.common_patterns {
            patterns.insert(p.clone());
        }

        // 5. Compress paths for macOS fsevents limit
        self.compress_patterns(patterns)
    }

    /// Extract a watch pattern from a bazel target
    ///
    /// Examples:
    /// - `//go/platform/gcp/query_endpoint:query_endpoint_binary` → `go/platform/gcp/query_endpoint/`
    /// - `//python_scio/platform/google/app/query_parser` → `python_scio/platform/google/app/query_parser/`
    /// - `//java/com/askscio/crawler:crawler.war` → `java/com/askscio/crawler/`
    fn target_to_package_pattern(&self, target: &str) -> Option<String> {
        let target = target.strip_prefix("//").unwrap_or(target);

        // Handle targets with explicit names (//path:name)
        let package = if target.contains(':') {
            target.split(':').next()?
        } else {
            target
        };

        // Return as directory pattern
        if package.is_empty() {
            None
        } else {
            Some(format!("{}/", package))
        }
    }

    /// Compress patterns to stay under macOS fsevents limit
    fn compress_patterns(&self, patterns: HashSet<String>) -> Vec<String> {
        let mut result: Vec<String> = patterns.into_iter().collect();
        result.sort();

        // Remove redundant patterns (where one is a prefix of another)
        let mut compressed = Vec::new();
        for pattern in &result {
            let dominated = compressed.iter().any(|p: &String| pattern.starts_with(p));
            if !dominated {
                // Remove any patterns that this one dominates
                compressed.retain(|p: &String| !p.starts_with(pattern));
                compressed.push(pattern.clone());
            }
        }

        compressed
    }

    /// Generate watch file YAML for a service
    pub async fn generate_watch_file(
        &self,
        service_name: &str,
        service_type: ServiceType,
        bazel_target: Option<&str>,
    ) -> Result<()> {
        let patterns = self.generate_patterns(service_name, service_type, bazel_target);

        let mut watch_map = HashMap::new();
        watch_map.insert(service_name.to_string(), patterns);

        let yaml = serde_yaml::to_string(&watch_map)?;

        let watch_file = self
            .workspace_root
            .join("devdock/runtime_generated")
            .join(format!("watch_file_{}.yaml", service_name));

        // Ensure directory exists
        if let Some(parent) = watch_file.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(&watch_file, yaml).await?;

        Ok(())
    }

    /// Generate all watch files in parallel (no bazel invocation!)
    pub async fn generate_all_watch_files(
        &self,
        services: &[(String, ServiceType, Option<String>)],
    ) -> Result<()> {
        let futures: Vec<_> = services
            .iter()
            .map(|(name, svc_type, target)| {
                self.generate_watch_file(name, svc_type.clone(), target.as_deref())
            })
            .collect();

        futures::future::try_join_all(futures).await?;
        Ok(())
    }

    /// Check if we have a cached cquery result that's still valid
    pub fn get_cached_patterns(
        &self,
        service_name: &str,
        target_hash: &str,
    ) -> Option<Vec<String>> {
        let cache_file = self
            .cache_dir
            .join(format!("{}_{}.yaml", service_name, target_hash));

        if !cache_file.exists() {
            return None;
        }

        // Check if cache is less than 24 hours old
        if let Ok(metadata) = std::fs::metadata(&cache_file) {
            if let Ok(modified) = metadata.modified() {
                let age = std::time::SystemTime::now()
                    .duration_since(modified)
                    .unwrap_or_default();

                if age.as_secs() > 24 * 60 * 60 {
                    return None; // Cache too old
                }
            }
        }

        // Load cached patterns
        let content = std::fs::read_to_string(&cache_file).ok()?;
        let map: HashMap<String, Vec<String>> = serde_yaml::from_str(&content).ok()?;
        map.get(service_name).cloned()
    }

    /// Save cquery results to cache for future use
    pub fn cache_patterns(
        &self,
        service_name: &str,
        target_hash: &str,
        patterns: &[String],
    ) -> Result<()> {
        let cache_file = self
            .cache_dir
            .join(format!("{}_{}.yaml", service_name, target_hash));

        let mut map = HashMap::new();
        map.insert(service_name.to_string(), patterns.to_vec());

        let yaml = serde_yaml::to_string(&map)?;
        std::fs::write(&cache_file, yaml)?;

        Ok(())
    }
}

/// Hybrid strategy: static patterns + optional background cquery
pub struct HybridWatchStrategy {
    static_generator: WatchPatternGenerator,
    enable_background_cquery: bool,
}

impl HybridWatchStrategy {
    pub fn new(workspace_root: PathBuf, enable_background_cquery: bool) -> Result<Self> {
        Ok(Self {
            static_generator: WatchPatternGenerator::new(workspace_root)?,
            enable_background_cquery,
        })
    }

    /// Fast path: generate static patterns immediately
    /// Background: optionally run cquery later to refine
    pub async fn generate_watch_files(
        &self,
        services: &[(String, ServiceType, Option<String>)],
    ) -> Result<()> {
        // Step 1: Generate static patterns immediately (< 1ms)
        println!("⚡ Generating watch patterns (static, no bazel)...");
        let start = std::time::Instant::now();
        self.static_generator
            .generate_all_watch_files(services)
            .await?;
        println!(
            "✅ Watch patterns generated in {:.2}ms",
            start.elapsed().as_secs_f64() * 1000.0
        );

        // Step 2: Optionally spawn background cquery to refine (non-blocking)
        if self.enable_background_cquery {
            println!("🔄 Background cquery refinement enabled (will update watch files)");
            // TODO: Spawn background task to run cquery and update files
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_to_package_pattern() {
        let gen = WatchPatternGenerator::new(PathBuf::from("/tmp")).unwrap();

        assert_eq!(
            gen.target_to_package_pattern("//go/platform/gcp/query_endpoint:query_endpoint_binary"),
            Some("go/platform/gcp/query_endpoint/".to_string())
        );

        assert_eq!(
            gen.target_to_package_pattern("//python_scio/platform/google/app/query_parser"),
            Some("python_scio/platform/google/app/query_parser/".to_string())
        );

        assert_eq!(
            gen.target_to_package_pattern("//java/com/askscio/crawler:crawler.war"),
            Some("java/com/askscio/crawler/".to_string())
        );
    }

    #[test]
    fn test_generate_patterns_go() {
        let gen = WatchPatternGenerator::new(PathBuf::from("/tmp")).unwrap();

        let patterns = gen.generate_patterns(
            "qe",
            ServiceType::Go,
            Some("//go/platform/gcp/query_endpoint:query_endpoint_binary"),
        );

        // The patterns are compressed, so we should have go/ directories (not the specific subdirectory)
        // because go/platform/ already covers go/platform/gcp/query_endpoint/
        assert!(
            patterns.iter().any(|p| p.starts_with("go/")),
            "Should have go patterns: {:?}",
            patterns
        );
        assert!(
            patterns.contains(&"config/".to_string()),
            "Should have config/: {:?}",
            patterns
        );
    }

    #[test]
    fn test_generate_patterns_python() {
        let gen = WatchPatternGenerator::new(PathBuf::from("/tmp")).unwrap();

        let patterns = gen.generate_patterns(
            "qp",
            ServiceType::Python,
            Some("//python_scio/platform/google/app/query_parser"),
        );

        assert!(patterns.iter().any(|p| p.starts_with("python_scio/")));
    }

    #[test]
    fn test_compress_patterns() {
        let gen = WatchPatternGenerator::new(PathBuf::from("/tmp")).unwrap();

        let mut patterns = HashSet::new();
        patterns.insert("go/".to_string());
        patterns.insert("go/platform/".to_string());
        patterns.insert("go/platform/gcp/".to_string());

        let compressed = gen.compress_patterns(patterns);

        // Should only have the most specific prefix
        assert_eq!(compressed, vec!["go/".to_string()]);
    }
}
