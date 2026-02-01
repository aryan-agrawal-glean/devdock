//! Google Cloud Storage fetcher for DevDock configuration files.
//!
//! Fetches dynamic configuration from GCS buckets like:
//! - `gs://config-{project}/` for dynamic INI configs
//! - `gs://sql-{project}/devdock/` for SQL dump files

use crate::ini_parser::ParsedIniConfig;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::{debug, info, warn};

/// GCS bucket URL patterns
const CONFIG_BUCKET_PATTERN: &str = "gs://config-{project}";
const SQL_BUCKET_PATTERN: &str = "gs://sql-{project}/devdock";

/// GCS Storage client for fetching DevDock configs
pub struct GcsStorage {
    project: String,
    cache_dir: PathBuf,
}

impl GcsStorage {
    /// Create a new GCS storage client
    pub fn new(project: &str, cache_dir: PathBuf) -> Self {
        Self { project: project.to_string(), cache_dir }
    }

    /// Create with default cache directory ($DEVDOCK_ROOT/cache or temp)
    pub fn with_default_cache(project: &str) -> Result<Self> {
        let cache_dir = if let Ok(devdock_root) = std::env::var("DEVDOCK_ROOT") {
            PathBuf::from(devdock_root).join("cache/gcs")
        } else {
            std::env::temp_dir().join("devdock/gcs_cache")
        };

        std::fs::create_dir_all(&cache_dir)
            .with_context(|| format!("Failed to create cache dir: {}", cache_dir.display()))?;

        Ok(Self::new(project, cache_dir))
    }

    /// Get the config bucket URL
    pub fn config_bucket(&self) -> String {
        CONFIG_BUCKET_PATTERN.replace("{project}", &self.project)
    }

    /// Get the SQL bucket URL
    pub fn sql_bucket(&self) -> String {
        SQL_BUCKET_PATTERN.replace("{project}", &self.project)
    }

    /// Fetch a file from GCS to local cache
    pub async fn fetch_file(&self, gcs_path: &str, local_name: &str) -> Result<PathBuf> {
        let local_path = self.cache_dir.join(local_name);

        debug!("Fetching {} to {}", gcs_path, local_path.display());

        let output = Command::new("gcloud")
            .args(["storage", "cp", gcs_path, local_path.to_str().unwrap()])
            .output()
            .await
            .context("Failed to run gcloud storage cp")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("gcloud storage cp failed: {stderr}");
        }

        Ok(local_path)
    }

    /// Fetch a file from GCS directly to a target path
    pub async fn fetch_to_path(&self, gcs_path: &str, target: &Path) -> Result<()> {
        debug!("Fetching {} to {}", gcs_path, target.display());

        // Ensure parent directory exists
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let output = Command::new("gcloud")
            .args(["storage", "cp", gcs_path, target.to_str().unwrap()])
            .output()
            .await
            .context("Failed to run gcloud storage cp")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("gcloud storage cp failed: {stderr}");
        }

        Ok(())
    }

    /// Fetch a file and return its contents as bytes
    pub async fn fetch_bytes(&self, gcs_path: &str) -> Result<Vec<u8>> {
        let temp_name = format!("temp_{}", uuid_simple());
        let local_path = self.fetch_file(gcs_path, &temp_name).await?;

        let bytes = tokio::fs::read(&local_path)
            .await
            .with_context(|| format!("Failed to read fetched file: {}", local_path.display()))?;

        // Clean up temp file
        let _ = tokio::fs::remove_file(&local_path).await;

        Ok(bytes)
    }

    /// Fetch a file and return its contents as string
    pub async fn fetch_string(&self, gcs_path: &str) -> Result<String> {
        let bytes = self.fetch_bytes(gcs_path).await?;
        String::from_utf8(bytes).context("GCS file contains invalid UTF-8")
    }

    /// Fetch multiple files in parallel to a target directory
    pub async fn fetch_files_parallel(
        &self,
        gcs_files: &[String],
        target_dir: &Path,
    ) -> Result<Vec<PathBuf>> {
        tokio::fs::create_dir_all(target_dir).await?;

        let futures: Vec<_> = gcs_files
            .iter()
            .map(|gcs_path| {
                let gcs_path = gcs_path.clone();
                let target_dir = target_dir.to_path_buf();
                async move {
                    let filename = gcs_path.rsplit('/').next().unwrap_or("unknown");
                    let target_path = target_dir.join(filename);
                    self.fetch_to_path(&gcs_path, &target_path).await?;
                    Ok::<_, anyhow::Error>(target_path)
                }
            })
            .collect();

        let results = futures::future::join_all(futures).await;

        let mut paths = Vec::new();
        for result in results {
            match result {
                Ok(path) => paths.push(path),
                Err(e) => warn!("Failed to fetch file: {}", e),
            }
        }

        Ok(paths)
    }

    /// List files in a GCS path
    pub async fn list_files(&self, gcs_path: &str) -> Result<Vec<String>> {
        let output = Command::new("gcloud")
            .args(["storage", "ls", gcs_path])
            .output()
            .await
            .context("Failed to run gcloud storage ls")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("gcloud storage ls failed: {stderr}");
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.lines().map(std::string::ToString::to_string).collect())
    }

    /// Check if a GCS path exists
    pub async fn exists(&self, gcs_path: &str) -> bool {
        Command::new("gcloud")
            .args(["storage", "ls", gcs_path])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Fetch and extract a tar.zst (zstandard compressed tar) archive from GCS
    ///
    /// Downloads the archive to a temp file, then decompresses and extracts to target_dir.
    pub async fn fetch_and_extract_tar_zst(&self, gcs_path: &str, target_dir: &Path) -> Result<()> {
        info!("Fetching tar.zst archive from {}", gcs_path);

        // Download to temp file
        let temp_name = format!("seed_{}.tar.zst", uuid_simple());
        let temp_path = self.fetch_file(gcs_path, &temp_name).await?;

        info!("Extracting to {}", target_dir.display());

        // Ensure target directory exists
        std::fs::create_dir_all(target_dir)?;

        // Decompress zstd and extract tar
        let zst_file = std::fs::File::open(&temp_path)
            .with_context(|| format!("Failed to open temp file: {}", temp_path.display()))?;

        let decoder =
            zstd::stream::Decoder::new(zst_file).context("Failed to create zstd decoder")?;

        let mut archive = tar::Archive::new(decoder);
        archive.unpack(target_dir).context("Failed to extract tar archive")?;

        // Clean up temp file
        let _ = std::fs::remove_file(&temp_path);

        info!("Extraction complete");
        Ok(())
    }

    /// Fetch and decompress a gzip file from GCS
    ///
    /// Downloads the .gz file and decompresses it to target_path.
    pub async fn fetch_and_decompress_gzip(
        &self,
        gcs_path: &str,
        target_path: &Path,
    ) -> Result<()> {
        use std::io::{Read, Write};

        info!("Fetching gzip file from {}", gcs_path);

        // Download to temp file
        let temp_name = format!("dump_{}.gz", uuid_simple());
        let temp_path = self.fetch_file(gcs_path, &temp_name).await?;

        info!("Decompressing to {}", target_path.display());

        // Ensure parent directory exists
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Decompress gzip
        let gz_file = std::fs::File::open(&temp_path)
            .with_context(|| format!("Failed to open temp file: {}", temp_path.display()))?;

        let mut decoder = flate2::read::GzDecoder::new(gz_file);
        let mut output_file = std::fs::File::create(target_path)
            .with_context(|| format!("Failed to create output file: {}", target_path.display()))?;

        // Use a buffer for efficient copying
        let mut buffer = vec![0u8; 64 * 1024]; // 64KB buffer
        loop {
            let bytes_read = decoder.read(&mut buffer).context("Failed to decompress gzip data")?;
            if bytes_read == 0 {
                break;
            }
            output_file
                .write_all(&buffer[..bytes_read])
                .context("Failed to write decompressed data")?;
        }

        // Clean up temp file
        let _ = std::fs::remove_file(&temp_path);

        info!("Decompression complete");
        Ok(())
    }
}

/// Dynamic INI config fetcher
pub struct DynamicIniLoader {
    storage: GcsStorage,
    devdock_root: PathBuf,
}

impl DynamicIniLoader {
    /// Create a new dynamic INI loader
    pub fn new(project: &str, devdock_root: PathBuf) -> Result<Self> {
        let storage = GcsStorage::with_default_cache(project)?;
        Ok(Self { storage, devdock_root })
    }

    /// Create with custom storage
    pub fn with_storage(storage: GcsStorage, devdock_root: PathBuf) -> Self {
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

    /// Fetch dynamic.ini from GCS (project-specific config overrides)
    pub async fn fetch_dynamic_ini(&self) -> Result<Option<ParsedIniConfig>> {
        let gcs_path = format!("{}/dynamic.ini", self.storage.config_bucket());

        if !self.storage.exists(&gcs_path).await {
            info!("No dynamic.ini found at {}", gcs_path);
            return Ok(None);
        }

        info!("Fetching dynamic.ini from {}", gcs_path);
        let content = self.storage.fetch_string(&gcs_path).await?;
        let config = ParsedIniConfig::parse_str(&content)?;

        Ok(Some(config))
    }

    /// Load merged config: default.ini + dynamic.ini from GCS
    pub async fn load_merged_config(&self) -> Result<ParsedIniConfig> {
        let mut config = self.load_default_ini().await?;

        // Try to fetch and merge dynamic config
        match self.fetch_dynamic_ini().await {
            Ok(Some(dynamic)) => {
                info!("Merging dynamic.ini overrides");
                config.merge(&dynamic);
            }
            Ok(None) => {
                debug!("No dynamic.ini to merge");
            }
            Err(e) => {
                warn!("Failed to fetch dynamic.ini: {} (using default only)", e);
            }
        }

        Ok(config)
    }

    /// Fetch a specific INI file from GCS
    pub async fn fetch_ini_from_gcs(&self, filename: &str) -> Result<ParsedIniConfig> {
        let gcs_path = format!("{}/{}", self.storage.config_bucket(), filename);
        info!("Fetching {} from GCS", gcs_path);

        let content = self.storage.fetch_string(&gcs_path).await?;
        ParsedIniConfig::parse_str(&content)
    }

    /// Get databases configured for frontend instance (like Python's process_instance)
    pub async fn get_frontend_databases(&self) -> Result<Vec<String>> {
        let config = self.load_default_ini().await?;

        let exclusions = [
            "test",
            "jqe_user_identity",
            "internal_admin",
            "internal_permissions",
            "central_eval",
            "announcements",
            "user_generated_content",
        ];

        let frontend_dbs: Vec<String> = config
            .get_sql_databases_by_instance("frontendInstance")
            .into_iter()
            .filter(|db| !exclusions.contains(&db.as_str()))
            .collect();

        Ok(frontend_dbs)
    }
}

/// SQL dump fetcher from GCS
pub struct SqlDumpFetcher {
    storage: GcsStorage,
}

impl SqlDumpFetcher {
    /// Create a new SQL dump fetcher
    pub fn new(project: &str) -> Result<Self> {
        let storage = GcsStorage::with_default_cache(project)?;
        Ok(Self { storage })
    }

    /// Fetch a SQL dump file for a database
    pub async fn fetch_sql_dump(&self, db_name: &str, target_dir: &Path) -> Result<PathBuf> {
        let gcs_path = format!("{}/{}.sql", self.storage.sql_bucket(), db_name);
        let target_path = target_dir.join(format!("{db_name}.sql"));

        info!("Fetching SQL dump for {} from {}", db_name, gcs_path);
        self.storage.fetch_to_path(&gcs_path, &target_path).await?;

        Ok(target_path)
    }

    /// Fetch multiple SQL dumps in parallel
    pub async fn fetch_sql_dumps_parallel(
        &self,
        db_names: &[String],
        target_dir: &Path,
    ) -> Result<Vec<PathBuf>> {
        let gcs_files: Vec<String> =
            db_names.iter().map(|db| format!("{}/{}.sql", self.storage.sql_bucket(), db)).collect();

        self.storage.fetch_files_parallel(&gcs_files, target_dir).await
    }

    /// Check if a SQL dump exists in GCS
    pub async fn dump_exists(&self, db_name: &str) -> bool {
        let gcs_path = format!("{}/{}.sql", self.storage.sql_bucket(), db_name);
        self.storage.exists(&gcs_path).await
    }
}

/// Simple UUID-like string generator (avoids adding uuid crate)
fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();

    format!("{:x}{:x}", duration.as_secs(), duration.subsec_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bucket_urls() {
        let storage = GcsStorage::new("glean-prod", PathBuf::from("/tmp"));
        assert_eq!(storage.config_bucket(), "gs://config-glean-prod");
        assert_eq!(storage.sql_bucket(), "gs://sql-glean-prod/devdock");
    }

    #[test]
    fn test_uuid_simple() {
        let uuid1 = uuid_simple();
        let uuid2 = uuid_simple();
        // Just verify it produces non-empty strings
        assert!(!uuid1.is_empty());
        assert!(!uuid2.is_empty());
    }
}
