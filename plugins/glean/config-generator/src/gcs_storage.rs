//! Google Cloud Storage fetcher for DevDock configuration files.
//!
//! Fetches dynamic configuration from GCS buckets like:
//! - `gs://config-{project}/` for dynamic INI configs
//! - `gs://sql-{project}/devdock/` for SQL dump files
//!
//! Uses the `google-cloud-storage` crate for direct API access with connection
//! pooling, replacing the previous `gcloud storage cp/ls` subprocess calls.

use crate::ini_parser::ParsedIniConfig;
use anyhow::{Context, Result};
use google_cloud_gax::paginator::ItemPaginator;
use google_cloud_storage::client::{Storage, StorageControl};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

/// GCS bucket URL patterns
const CONFIG_BUCKET_PATTERN: &str = "gs://config-{project}";
const SQL_BUCKET_PATTERN: &str = "gs://sql-{project}/devdock";

/// Parse a `gs://bucket-name/object/path` URI into `("bucket-name", "object/path")`.
fn parse_gcs_uri(uri: &str) -> Result<(&str, &str)> {
    let rest = uri.strip_prefix("gs://").with_context(|| format!("Invalid GCS URI: {uri}"))?;
    let (bucket, object) =
        rest.split_once('/').with_context(|| format!("GCS URI missing object path: {uri}"))?;
    Ok((bucket, object))
}

/// GCS Storage client for fetching DevDock configs
pub struct GcsStorage {
    client: Storage,
    control: StorageControl,
    project: String,
    cache_dir: PathBuf,
}

impl GcsStorage {
    /// Create a new GCS storage client
    pub async fn new(project: &str, cache_dir: PathBuf) -> Result<Self> {
        let client =
            Storage::builder().build().await.context("Failed to build GCS Storage client")?;
        let control = StorageControl::builder()
            .build()
            .await
            .context("Failed to build GCS StorageControl client")?;
        Ok(Self { client, control, project: project.to_string(), cache_dir })
    }

    /// Create with default cache directory ($DEVDOCK_ROOT/cache or temp)
    pub async fn with_default_cache(project: &str) -> Result<Self> {
        let cache_dir = if let Ok(devdock_root) = std::env::var("DEVDOCK_ROOT") {
            PathBuf::from(devdock_root).join("cache/gcs")
        } else {
            std::env::temp_dir().join("devdock/gcs_cache")
        };

        std::fs::create_dir_all(&cache_dir)
            .with_context(|| format!("Failed to create cache dir: {}", cache_dir.display()))?;

        Self::new(project, cache_dir).await
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

        let (bucket, object) = parse_gcs_uri(gcs_path)?;
        let mut reader = self
            .client
            .read_object(format!("projects/_/buckets/{bucket}"), object)
            .send()
            .await
            .with_context(|| format!("Failed to read object: {gcs_path}"))?;

        let mut file = tokio::fs::File::create(&local_path)
            .await
            .with_context(|| format!("Failed to create local file: {}", local_path.display()))?;

        while let Some(data) = reader.next().await.transpose().context("Error streaming object")? {
            file.write_all(&data).await?;
        }
        file.flush().await?;

        Ok(local_path)
    }

    /// Fetch a file from GCS directly to a target path
    pub async fn fetch_to_path(&self, gcs_path: &str, target: &Path) -> Result<()> {
        debug!("Fetching {} to {}", gcs_path, target.display());

        // Ensure parent directory exists
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let (bucket, object) = parse_gcs_uri(gcs_path)?;
        let mut reader = self
            .client
            .read_object(format!("projects/_/buckets/{bucket}"), object)
            .send()
            .await
            .with_context(|| format!("Failed to read object: {gcs_path}"))?;

        let mut file = tokio::fs::File::create(target)
            .await
            .with_context(|| format!("Failed to create target file: {}", target.display()))?;

        while let Some(data) = reader.next().await.transpose().context("Error streaming object")? {
            file.write_all(&data).await?;
        }
        file.flush().await?;

        Ok(())
    }

    /// Fetch a file and return its contents as bytes
    pub async fn fetch_bytes(&self, gcs_path: &str) -> Result<Vec<u8>> {
        let (bucket, object) = parse_gcs_uri(gcs_path)?;
        let mut reader = self
            .client
            .read_object(format!("projects/_/buckets/{bucket}"), object)
            .send()
            .await
            .with_context(|| format!("Failed to read object: {gcs_path}"))?;

        let mut bytes = Vec::new();
        while let Some(data) = reader.next().await.transpose().context("Error streaming object")? {
            bytes.extend_from_slice(&data);
        }

        Ok(bytes)
    }

    /// Fetch a file and return its contents as string
    pub async fn fetch_string(&self, gcs_path: &str) -> Result<String> {
        let bytes = self.fetch_bytes(gcs_path).await?;
        String::from_utf8(bytes).context("GCS file contains invalid UTF-8")
    }

    /// List files in a GCS path
    pub async fn list_files(&self, gcs_path: &str) -> Result<Vec<String>> {
        let (bucket, prefix) = parse_gcs_uri(gcs_path)?;

        let mut objects = self
            .control
            .list_objects()
            .set_parent(format!("projects/_/buckets/{bucket}"))
            .set_prefix(prefix)
            .by_item();

        let mut files = Vec::new();
        while let Some(object) =
            objects.next().await.transpose().context("Error listing objects")?
        {
            // Reconstruct the full gs:// URI for compatibility with existing callers
            files.push(format!("gs://{}/{}", bucket, object.name));
        }

        Ok(files)
    }

    /// Check if a GCS path exists
    pub async fn exists(&self, gcs_path: &str) -> bool {
        let (bucket, object) = match parse_gcs_uri(gcs_path) {
            Ok(v) => v,
            Err(_) => return false,
        };

        // Try to read the first chunk; if it succeeds the object exists
        self.client.read_object(format!("projects/_/buckets/{bucket}"), object).send().await.is_ok()
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
    pub async fn new(project: &str, devdock_root: PathBuf) -> Result<Self> {
        let storage = GcsStorage::with_default_cache(project).await?;
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
        // config_bucket and sql_bucket don't need the client, so test via parse_gcs_uri
        let config = CONFIG_BUCKET_PATTERN.replace("{project}", "glean-prod");
        assert_eq!(config, "gs://config-glean-prod");

        let sql = SQL_BUCKET_PATTERN.replace("{project}", "glean-prod");
        assert_eq!(sql, "gs://sql-glean-prod/devdock");
    }

    #[test]
    fn test_parse_gcs_uri() {
        let (bucket, object) = parse_gcs_uri("gs://my-bucket/some/path/file.txt").unwrap();
        assert_eq!(bucket, "my-bucket");
        assert_eq!(object, "some/path/file.txt");

        let (bucket, object) = parse_gcs_uri("gs://config-glean-prod/dynamic.ini").unwrap();
        assert_eq!(bucket, "config-glean-prod");
        assert_eq!(object, "dynamic.ini");

        assert!(parse_gcs_uri("s3://wrong/path").is_err());
        assert!(parse_gcs_uri("gs://no-slash").is_err());
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
