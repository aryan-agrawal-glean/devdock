//! AWS S3 Storage fetcher for DevDock configuration files.
//!
//! Fetches dynamic configuration from S3 buckets like:
//! - `s3://config-{aws_account_id}/` for dynamic INI configs
//! - `s3://sql-{aws_account_id}/devdock/` for SQL dump files

use crate::ini_parser::ParsedIniConfig;
use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::process::Command;
use tracing::{debug, info, warn};

/// S3 bucket URL patterns
const CONFIG_BUCKET_PATTERN: &str = "s3://config-{account_id}";
const SQL_BUCKET_PATTERN: &str = "s3://sql-{account_id}/devdock";

/// S3 Storage client for fetching DevDock configs
pub struct S3Storage {
    account_id: String,
    cache_dir: PathBuf,
    region: String,
}

impl S3Storage {
    /// Create a new S3 storage client
    pub fn new(account_id: &str, cache_dir: PathBuf, region: Option<String>) -> Self {
        Self {
            account_id: account_id.to_string(),
            cache_dir,
            region: region.unwrap_or_else(|| "us-east-1".to_string()),
        }
    }

    /// Create with default cache directory
    pub fn with_default_cache(account_id: &str, region: Option<String>) -> Result<Self> {
        let cache_dir = if let Ok(devdock_root) = std::env::var("DEVDOCK_ROOT") {
            PathBuf::from(devdock_root).join("cache/s3")
        } else {
            std::env::temp_dir().join("devdock/s3_cache")
        };

        std::fs::create_dir_all(&cache_dir)
            .with_context(|| format!("Failed to create cache dir: {}", cache_dir.display()))?;

        Ok(Self::new(account_id, cache_dir, region))
    }

    /// Get the config bucket URL
    pub fn config_bucket(&self) -> String {
        CONFIG_BUCKET_PATTERN.replace("{account_id}", &self.account_id)
    }

    /// Get the SQL bucket URL
    pub fn sql_bucket(&self) -> String {
        SQL_BUCKET_PATTERN.replace("{account_id}", &self.account_id)
    }

    /// Check if a file exists in S3
    pub async fn exists(&self, s3_path: &str) -> bool {
        let output = Command::new("aws")
            .args(["s3", "ls", s3_path, "--region", &self.region])
            .output()
            .await;

        matches!(output, Ok(o) if o.status.success())
    }

    /// Fetch a file from S3 and return its contents as a string
    pub async fn fetch_string(&self, s3_path: &str) -> Result<String> {
        debug!("Fetching {} from S3", s3_path);

        let output = Command::new("aws")
            .args(["s3", "cp", s3_path, "-", "--region", &self.region])
            .output()
            .await
            .context("Failed to run aws s3 cp")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("aws s3 cp failed: {stderr}");
        }

        let content =
            String::from_utf8(output.stdout).context("S3 file content is not valid UTF-8")?;

        Ok(content)
    }

    /// Fetch a file from S3 to local cache
    pub async fn fetch_file(&self, s3_path: &str, local_name: &str) -> Result<PathBuf> {
        let local_path = self.cache_dir.join(local_name);

        debug!("Fetching {} to {}", s3_path, local_path.display());

        // Ensure parent directory exists
        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let output = Command::new("aws")
            .args(["s3", "cp", s3_path, local_path.to_str().unwrap(), "--region", &self.region])
            .output()
            .await
            .context("Failed to run aws s3 cp")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("aws s3 cp failed: {stderr}");
        }

        Ok(local_path)
    }

    /// Fetch a file and return its contents as bytes
    pub async fn fetch_bytes(&self, s3_path: &str) -> Result<Vec<u8>> {
        debug!("Fetching {} from S3 as bytes", s3_path);

        let output = Command::new("aws")
            .args(["s3", "cp", s3_path, "-", "--region", &self.region])
            .output()
            .await
            .context("Failed to run aws s3 cp")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("aws s3 cp failed: {stderr}");
        }

        Ok(output.stdout)
    }
}

/// Dynamic INI loader for S3
pub struct S3DynamicIniLoader {
    storage: S3Storage,
    devdock_root: PathBuf,
}

impl S3DynamicIniLoader {
    /// Create a new S3 dynamic INI loader
    pub fn new(account_id: &str, devdock_root: PathBuf, region: Option<String>) -> Result<Self> {
        let storage = S3Storage::with_default_cache(account_id, region)?;
        Ok(Self { storage, devdock_root })
    }

    /// Create with custom storage
    pub fn with_storage(storage: S3Storage, devdock_root: PathBuf) -> Self {
        Self { storage, devdock_root }
    }

    /// Get the path to the local default.ini
    pub fn default_ini_path(&self) -> PathBuf {
        self.devdock_root.join("config/default.ini")
    }

    /// Load the local default.ini file
    pub async fn load_default_ini(&self) -> Result<ParsedIniConfig> {
        let path = self.default_ini_path();

        if !path.exists() {
            anyhow::bail!("default.ini not found at: {}", path.display());
        }

        let content = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("Failed to read {}", path.display()))?;

        ParsedIniConfig::parse_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))
    }

    /// Fetch dynamic.ini from S3 (account-specific config overrides)
    pub async fn fetch_dynamic_ini(&self) -> Result<Option<ParsedIniConfig>> {
        let s3_path = format!("{}/dynamic.ini", self.storage.config_bucket());

        if !self.storage.exists(&s3_path).await {
            info!("No dynamic.ini found at {}", s3_path);
            return Ok(None);
        }

        info!("Fetching dynamic.ini from {}", s3_path);
        let content = self.storage.fetch_string(&s3_path).await?;
        let config = ParsedIniConfig::parse_str(&content)?;

        Ok(Some(config))
    }

    /// Load merged config: default.ini + dynamic.ini from S3
    pub async fn load_merged_config(&self) -> Result<ParsedIniConfig> {
        let mut config = self.load_default_ini().await?;

        // Try to fetch and merge dynamic config
        match self.fetch_dynamic_ini().await {
            Ok(Some(dynamic)) => {
                info!("Merging dynamic.ini overrides from S3");
                config.merge(&dynamic);
            }
            Ok(None) => {
                debug!("No dynamic.ini to merge from S3");
            }
            Err(e) => {
                warn!("Failed to fetch dynamic.ini from S3: {} (using default only)", e);
            }
        }

        Ok(config)
    }
}
