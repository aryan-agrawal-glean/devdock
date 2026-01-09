//! Bazel query integration for generating watch files.

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tokio::process::Command;

/// Bazel query engine for generating watch files
#[derive(Clone)]
pub struct BazelQueryEngine {
    workspace_root: PathBuf,
    use_remote_bazelrc: bool,
    silent: bool,
}

impl BazelQueryEngine {
    pub fn new(workspace_root: PathBuf) -> Self {
        let use_remote_bazelrc = cfg!(target_os = "linux");
        Self {
            workspace_root,
            use_remote_bazelrc,
            silent: false,
        }
    }

    /// Create a silent engine (no console output, for background tasks)
    pub fn new_silent(workspace_root: PathBuf) -> Self {
        let use_remote_bazelrc = cfg!(target_os = "linux");
        Self {
            workspace_root,
            use_remote_bazelrc,
            silent: true,
        }
    }

    /// Generate all watch files in parallel
    pub async fn generate_watch_files(
        &self,
        services_with_targets: HashMap<String, String>,
    ) -> Result<()> {
        if services_with_targets.is_empty() {
            if !self.silent {
                println!("      └─ No bazel targets to query");
            }
            return Ok(());
        }

        if !self.silent {
            println!(
                "      └─ Querying bazel for {} service(s): {:?}",
                services_with_targets.len(),
                services_with_targets.keys().collect::<Vec<_>>()
            );
        }

        let total_start = std::time::Instant::now();
        let mut futures = Vec::new();

        // Generate all watch files in parallel
        for (service_name, target) in services_with_targets {
            let engine = self.clone();
            let name = service_name.clone();

            futures.push(tokio::spawn(async move {
                let start = std::time::Instant::now();
                let result = engine.generate_watch_file(&name, &target).await;
                if !engine.silent {
                    println!(
                        "         └─ bazel cquery {} completed in {:.2}s",
                        name,
                        start.elapsed().as_secs_f64()
                    );
                }
                result
            }));
        }

        // Wait for all to complete
        let results = futures::future::join_all(futures).await;

        // Check for errors
        let mut success_count = 0;
        let mut error_count = 0;
        for result in results {
            match result {
                Ok(Ok(_)) => success_count += 1,
                Ok(Err(e)) => {
                    error_count += 1;
                    if !self.silent {
                        eprintln!("         └─ Warning: Failed to generate watch file: {}", e);
                    }
                }
                Err(e) => {
                    error_count += 1;
                    if !self.silent {
                        eprintln!("         └─ Warning: Task panicked: {}", e);
                    }
                }
            }
        }

        if !self.silent {
            println!(
                "      └─ Bazel queries: {} success, {} errors in {:.2}s",
                success_count,
                error_count,
                total_start.elapsed().as_secs_f64()
            );
        }

        Ok(())
    }

    /// Generate a single watch file for a service
    async fn generate_watch_file(&self, service_name: &str, target: &str) -> Result<()> {
        let mut cmd = Command::new("bazel");

        if self.use_remote_bazelrc {
            cmd.arg("--nohome_rc");
            cmd.arg(format!(
                "--bazelrc={}/.devdock.remote.bazelrc",
                self.workspace_root.display()
            ));
        }

        cmd.arg("cquery");
        cmd.arg("--output=label");
        cmd.arg(format!(
            "filter('^//', kind('source file', deps(set({}))))",
            target
        ));
        cmd.current_dir(&self.workspace_root);

        let output = cmd.output().await.context("Failed to run bazel cquery")?;

        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "Bazel cquery failed for {}: {}",
                service_name,
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Parse output
        let stdout = String::from_utf8(output.stdout)?;
        let files: Vec<String> = stdout
            .lines()
            .filter(|line| !line.is_empty() && line.starts_with("//"))
            .map(|line| {
                line.split_whitespace()
                    .next()
                    .unwrap_or("")
                    .strip_prefix("//")
                    .unwrap_or(line)
                    .replace(":", "/")
            })
            .collect();

        // Compress paths (macOS fsevents limit)
        let compressed = self.compress_file_paths(&files)?;

        // Write YAML
        let mut watch_map = HashMap::new();
        watch_map.insert(service_name, compressed);

        let watch_file = self
            .workspace_root
            .join("devdock/runtime_generated")
            .join(format!("watch_file_{}.yaml", service_name));

        let yaml = serde_yaml::to_string(&watch_map)?;
        tokio::fs::write(watch_file, yaml).await?;

        Ok(())
    }

    /// Compress file paths to stay under macOS fsevents limit (4047 files)
    fn compress_file_paths(&self, files: &[String]) -> Result<Vec<String>> {
        // Get all non-code directories
        let exclude_dirs: HashSet<&str> = ["go", "java", "python_scio"].into_iter().collect();

        let all_dirs: HashSet<String> = std::fs::read_dir(&self.workspace_root)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|d| !exclude_dirs.contains(d.as_str()))
            .collect();

        let mut compressed = HashSet::new();

        for file in files {
            // If starts with non-code directory, just use top-level
            if all_dirs.iter().any(|d| file.starts_with(d)) {
                if let Some(first) = file.split('/').next() {
                    compressed.insert(first.to_string());
                }
                continue;
            }

            // For code directories, compress to 3-4 levels deep
            let parts: Vec<&str> = file.split('/').collect();
            let java_offset = if file.starts_with("java") { 2 } else { 0 };

            if parts.len() >= 3 + java_offset {
                let compressed_path = parts[..3 + java_offset].join("/");
                compressed.insert(compressed_path);
            } else {
                compressed.insert(file.to_string());
            }
        }

        let mut result: Vec<String> = compressed.into_iter().collect();
        result.sort();
        Ok(result)
    }
}
