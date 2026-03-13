//! OpenSearch index copy operations for DevDock.
//!
//! This module replaces:
//! - devdock/config/infra/opensearch/scripts/copy_indexes.py

use crate::gcs_storage::DynamicIniLoader;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tracing::{error, info, warn};

/// Local Elasticsearch port
const LOCAL_ELASTICSEARCH_PORT: u16 = 9200;

/// Tunnel port for remote access
const TUNNEL_PORT: u16 = 9201;

/// OpenSearch index copier
pub struct OpenSearchCopier {
    #[allow(dead_code)]
    project: String,
    server_url: String,
    private_setup: bool,
}

impl OpenSearchCopier {
    /// Create a new OpenSearch copier from project config
    pub async fn new(project: &str) -> Result<Self> {
        let devdock_root =
            std::env::var("DEVDOCK_ROOT").context("DEVDOCK_ROOT environment variable not set")?;
        let devdock_root = PathBuf::from(devdock_root);

        let ini_loader = DynamicIniLoader::new(project, devdock_root).await?;
        let config = ini_loader.load_merged_config().await?;

        let server_url = config
            .get("indexing", "server.url")
            .cloned()
            .unwrap_or_else(|| "http://localhost:9200".to_string());

        let private_setup = config
            .get("deploy", "deploy_service.private_setup")
            .is_some_and(|s| s.to_lowercase() == "true");

        Ok(Self { project: project.to_string(), server_url, private_setup })
    }

    /// Create with explicit config
    pub fn with_config(project: &str, server_url: &str, private_setup: bool) -> Self {
        Self { project: project.to_string(), server_url: server_url.to_string(), private_setup }
    }

    /// Check if Elasticsearch is healthy on a given port
    fn health_check(port: u16) -> bool {
        let url = format!("http://localhost:{port}/");

        match reqwest::blocking::get(&url) {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    /// Wait for Elasticsearch to be healthy
    fn wait_for_health(port: u16, timeout: Duration) -> Result<()> {
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            if Self::health_check(port) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_secs(1));
        }

        anyhow::bail!("Timeout waiting for Elasticsearch on port {port}")
    }

    /// Start SSH tunnel to remote Elasticsearch
    fn start_tunnel(&self) -> Result<Child> {
        let host = self
            .server_url
            .strip_prefix("http://")
            .or_else(|| self.server_url.strip_prefix("https://"))
            .unwrap_or(&self.server_url);

        let tunnel_arg = if self.private_setup { "--internal-ip" } else { "--tunnel-through-iap" };

        info!("Starting SSH tunnel to {} ({})", host, tunnel_arg);

        let child = Command::new("gcloud")
            .args([
                "compute",
                "ssh",
                "--zone",
                "us-central1-a",
                tunnel_arg,
                "bastion",
                "--",
                "-NL",
                &format!("localhost:{TUNNEL_PORT}:{host}"),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to start SSH tunnel")?;

        // Wait for tunnel to be ready
        Self::wait_for_health(TUNNEL_PORT, Duration::from_secs(60))?;

        info!("SSH tunnel established on port {}", TUNNEL_PORT);
        Ok(child)
    }

    /// Kill tunnel process
    fn stop_tunnel(mut process: Child) {
        let _ = process.kill();
        let _ = process.wait();
    }

    /// Fetch data chunk using scroll API
    fn fetch_data_chunk(
        &self,
        _index: &str,
        query: &serde_json::Value,
        size: usize,
        scroll: &str,
        scroll_id: Option<&str>,
    ) -> Result<serde_json::Value> {
        let client = reqwest::blocking::Client::new();
        let source_url = format!("http://localhost:{TUNNEL_PORT}");

        let response = if let Some(sid) = scroll_id {
            // Continue scroll
            let body = serde_json::json!({
                "scroll_id": sid,
                "scroll": scroll
            });

            client
                .post(format!("{source_url}/_search/scroll"))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()?
        } else {
            // Initial scroll request
            let body = serde_json::json!({
                "query": query
            });

            client
                .post(format!("{source_url}/_search?scroll={scroll}&size={size}"))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()?
        };

        response.json().context("Failed to parse scroll response")
    }

    /// Index data chunk using bulk API
    fn index_data_chunk(&self, docs: &[serde_json::Value], target_index: &str) -> Result<()> {
        if docs.is_empty() {
            return Ok(());
        }

        let client = reqwest::blocking::Client::new();
        let dest_url = format!("http://localhost:{LOCAL_ELASTICSEARCH_PORT}");

        // Build bulk request body
        let mut bulk_data = String::new();
        for doc in docs {
            let id = doc.get("_id").and_then(|v| v.as_str()).unwrap_or("unknown");
            let source = doc.get("_source").unwrap_or(doc);

            let action = serde_json::json!({
                "index": {
                    "_index": target_index,
                    "_id": id
                }
            });

            bulk_data.push_str(&serde_json::to_string(&action)?);
            bulk_data.push('\n');
            bulk_data.push_str(&serde_json::to_string(source)?);
            bulk_data.push('\n');
        }
        bulk_data.push('\n');

        let response = client
            .post(format!("{dest_url}/_bulk"))
            .header("Content-Type", "application/x-ndjson")
            .body(bulk_data)
            .send()?;

        if !response.status().is_success() {
            let body: serde_json::Value = response.json()?;
            warn!("Bulk indexing had errors: {:?}", body);
        }

        Ok(())
    }

    /// Chunked reindex from remote to local
    fn chunked_reindex(
        &self,
        source_index: &str,
        target_index: &str,
        query: &serde_json::Value,
        size: usize,
        scroll: &str,
        max_docs: usize,
    ) -> Result<usize> {
        let mut total_docs = 0;
        let max_chunks = if max_docs > 0 { max_docs.div_ceil(size) } else { usize::MAX };

        // Initial fetch
        let response = self.fetch_data_chunk(source_index, query, size, scroll, None)?;

        let mut scroll_id = response.get("_scroll_id").and_then(|v| v.as_str()).map(String::from);

        let mut docs: Vec<serde_json::Value> = response
            .get("hits")
            .and_then(|h| h.get("hits"))
            .and_then(|h| h.as_array())
            .cloned()
            .unwrap_or_default();

        let mut chunk_count = 0;

        while !docs.is_empty() && chunk_count < max_chunks {
            self.index_data_chunk(&docs, target_index)?;
            total_docs += docs.len();
            chunk_count += 1;

            info!(
                "Indexed chunk {} ({} docs so far) for {}",
                chunk_count, total_docs, target_index
            );

            // Get next chunk
            if let Some(ref sid) = scroll_id {
                let response =
                    self.fetch_data_chunk(source_index, query, size, scroll, Some(sid))?;

                scroll_id = response.get("_scroll_id").and_then(|v| v.as_str()).map(String::from);

                docs = response
                    .get("hits")
                    .and_then(|h| h.get("hits"))
                    .and_then(|h| h.as_array())
                    .cloned()
                    .unwrap_or_default();
            } else {
                break;
            }
        }

        Ok(total_docs)
    }

    /// Copy index from remote to local
    pub fn copy_index(
        &self,
        remote_index: &str,
        local_index: &str,
        query: &str,
        max_docs: usize,
    ) -> Result<usize> {
        info!(
            "Copying index {} -> {} (max_docs: {})",
            remote_index,
            local_index,
            if max_docs == 0 { "unlimited".to_string() } else { max_docs.to_string() }
        );

        // Start tunnel
        let tunnel = self.start_tunnel()?;

        // Parse query
        let query_json: serde_json::Value =
            serde_json::from_str(query).unwrap_or_else(|_| serde_json::json!({"match_all": {}}));

        // Perform chunked reindex
        let result =
            self.chunked_reindex(remote_index, local_index, &query_json, 500, "2m", max_docs);

        // Stop tunnel
        Self::stop_tunnel(tunnel);

        result
    }

    /// Copy multiple indexes
    pub fn copy_indexes(
        &self,
        indexes: &[String],
        query: &str,
        max_docs: usize,
    ) -> Result<HashMap<String, usize>> {
        let mut results = HashMap::new();

        for index in indexes {
            match self.copy_index(index, index, query, max_docs) {
                Ok(count) => {
                    info!("✅ Copied {} documents for index {}", count, index);
                    results.insert(index.clone(), count);
                }
                Err(e) => {
                    error!("❌ Failed to copy index {}: {}", index, e);
                }
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_copier_creation() {
        let copier =
            OpenSearchCopier::with_config("test-project", "http://elasticsearch:9200", false);
        assert_eq!(copier.project, "test-project");
        assert_eq!(copier.server_url, "http://elasticsearch:9200");
        assert!(!copier.private_setup);
    }
}
