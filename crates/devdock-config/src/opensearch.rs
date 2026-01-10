//! OpenSearch/Elasticsearch utilities for DevDock.
//!
//! This module replaces python_scio OpenSearch dependencies:
//! - python_scio.core.elastic_common (get_elastic_namespace, namespace mappings)
//! - python_scio.core.config_constants (IndexingConfig)

use crate::constants::{
    IndexingConfig, ELASTICSEARCH_NAMESPACES, ELASTIC_EXPERIMENTAL_NAMESPACE_1,
    ELASTIC_EXPERIMENTAL_NAMESPACE_2, ELASTIC_EXPERIMENTAL_NODE_POOL_1,
    ELASTIC_EXPERIMENTAL_NODE_POOL_2,
};
use crate::ini_parser::ParsedIniConfig;
use anyhow::Result;
use std::collections::HashMap;
use tracing::debug;

/// Mapping from node pool name to namespace
fn get_node_pool_to_namespace() -> HashMap<String, String> {
    let mut mapping = HashMap::new();

    // Standard node pools (1-12)
    for i in 1..=12 {
        let pool = get_elastic_node_pool_name_from_number(i);
        mapping.insert(pool, ELASTICSEARCH_NAMESPACES[i - 1].to_string());
    }

    // Experimental node pools
    mapping.insert(
        ELASTIC_EXPERIMENTAL_NODE_POOL_1.to_string(),
        ELASTIC_EXPERIMENTAL_NAMESPACE_1.to_string(),
    );
    mapping.insert(
        ELASTIC_EXPERIMENTAL_NODE_POOL_2.to_string(),
        ELASTIC_EXPERIMENTAL_NAMESPACE_2.to_string(),
    );

    mapping
}

/// Mapping from namespace to node pool name
fn get_namespace_to_node_pool() -> HashMap<String, String> {
    let pool_to_ns = get_node_pool_to_namespace();
    pool_to_ns.into_iter().map(|(k, v)| (v, k)).collect()
}

/// Get node pool name from number (1-12)
///
/// Returns:
/// - "elastic-node-pool-1" through "elastic-node-pool-12" on GCP
fn get_elastic_node_pool_name_from_number(number: usize) -> String {
    format!("elastic-node-pool-{number}")
}

/// Get namespace from node pool name
///
/// Replaces: python_scio.core.elastic_common.get_elastic_namespace
pub fn get_elastic_namespace(node_pool_name: &str) -> Result<String> {
    let mapping = get_node_pool_to_namespace();

    mapping.get(node_pool_name).cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "Invalid node pool [{}]. Valid node pools: {:?}",
            node_pool_name,
            mapping.keys().collect::<Vec<_>>()
        )
    })
}

/// Get node pool name from namespace
pub fn get_elastic_node_pool(namespace: &str) -> Result<String> {
    let mapping = get_namespace_to_node_pool();

    mapping.get(namespace).cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "Invalid namespace [{}]. Valid namespaces: {:?}",
            namespace,
            mapping.keys().collect::<Vec<_>>()
        )
    })
}

/// Get namespace suffix from full namespace name
///
/// Example: "elasticsearch-1-namespace" -> "1"
pub fn get_namespace_suffix(namespace: &str) -> String {
    let parts: Vec<&str> = namespace.split('-').collect();

    if parts.len() >= 3 && parts[0] == "elasticsearch" && parts[2] == "namespace" {
        parts[1].to_string()
    } else {
        String::new()
    }
}

/// OpenSearch configuration from INI
#[derive(Debug, Clone)]
pub struct OpenSearchConfig {
    /// Node pool name (e.g., "elastic-node-pool-1")
    pub node_pool: String,
    /// Namespace derived from node pool
    pub namespace: String,
    /// Server URL if configured
    pub server_url: Option<String>,
    /// Whether this is staging
    pub is_staging: bool,
}

impl OpenSearchConfig {
    /// Load OpenSearch configuration from INI config
    pub fn from_config(config: &ParsedIniConfig, is_staging: bool) -> Result<Self> {
        let node_pool_key = if is_staging {
            IndexingConfig::STAGING_NODE_POOL_NAME
        } else {
            IndexingConfig::NODE_POOL_NAME
        };

        let node_pool = config
            .get("indexing", node_pool_key)
            .cloned()
            .unwrap_or_else(|| "elastic-node-pool-1".to_string());

        let namespace = get_elastic_namespace(&node_pool)
            .unwrap_or_else(|_| ELASTICSEARCH_NAMESPACES[0].to_string());

        let server_url = config.get("indexing", IndexingConfig::SERVER_URL).cloned();

        debug!(
            "OpenSearch config: node_pool={}, namespace={}, server_url={:?}",
            node_pool, namespace, server_url
        );

        Ok(Self { node_pool, namespace, server_url, is_staging })
    }
}

/// Get OpenSearch server URL from config
pub fn get_opensearch_server_url(config: &ParsedIniConfig, is_staging: bool) -> Option<String> {
    if is_staging {
        config.get("indexing", "staging.serverUrl").cloned()
    } else {
        config.get("indexing", IndexingConfig::SERVER_URL).cloned()
    }
}

/// Index types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexType {
    /// Document index
    Document,
    /// Entity index
    Entity,
    /// Knowledge graph index
    KnowledgeGraph,
}

impl IndexType {
    /// Get default index name for this type
    pub fn default_name(&self) -> &'static str {
        match self {
            IndexType::Document => "index-1",
            IndexType::Entity => "entity-index-1",
            IndexType::KnowledgeGraph => "kg-index-1",
        }
    }

    /// Get short code for this type
    pub fn code(&self) -> &'static str {
        match self {
            IndexType::Document => "doc",
            IndexType::Entity => "entity",
            IndexType::KnowledgeGraph => "kg",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_pool_name_from_number() {
        assert_eq!(get_elastic_node_pool_name_from_number(1), "elastic-node-pool-1");
        assert_eq!(get_elastic_node_pool_name_from_number(12), "elastic-node-pool-12");
    }

    #[test]
    fn test_get_elastic_namespace() {
        assert_eq!(
            get_elastic_namespace("elastic-node-pool-1").unwrap(),
            "elasticsearch-1-namespace"
        );
        assert_eq!(
            get_elastic_namespace("elastic-node-pool-12").unwrap(),
            "elasticsearch-12-namespace"
        );
        assert_eq!(
            get_elastic_namespace("elastic-exp-node-pool-1").unwrap(),
            "elastic-exp-namespace-1"
        );
    }

    #[test]
    fn test_get_elastic_namespace_invalid() {
        assert!(get_elastic_namespace("invalid-pool").is_err());
    }

    #[test]
    fn test_get_elastic_node_pool() {
        assert_eq!(
            get_elastic_node_pool("elasticsearch-1-namespace").unwrap(),
            "elastic-node-pool-1"
        );
        assert_eq!(
            get_elastic_node_pool("elasticsearch-12-namespace").unwrap(),
            "elastic-node-pool-12"
        );
    }

    #[test]
    fn test_get_namespace_suffix() {
        assert_eq!(get_namespace_suffix("elasticsearch-1-namespace"), "1");
        assert_eq!(get_namespace_suffix("elasticsearch-12-namespace"), "12");
        assert_eq!(get_namespace_suffix("invalid"), "");
    }

    #[test]
    fn test_opensearch_config_from_ini() {
        let ini = r"
[indexing]
index.nodePool = elastic-node-pool-3
index.serverUrl = http://localhost:9200
";
        let config = ParsedIniConfig::parse_str(ini).unwrap();
        let os_config = OpenSearchConfig::from_config(&config, false).unwrap();

        assert_eq!(os_config.node_pool, "elastic-node-pool-3");
        assert_eq!(os_config.namespace, "elasticsearch-3-namespace");
        assert_eq!(os_config.server_url, Some("http://localhost:9200".to_string()));
    }

    #[test]
    fn test_index_type() {
        assert_eq!(IndexType::Document.default_name(), "index-1");
        assert_eq!(IndexType::Entity.default_name(), "entity-index-1");
        assert_eq!(IndexType::KnowledgeGraph.default_name(), "kg-index-1");

        assert_eq!(IndexType::Document.code(), "doc");
        assert_eq!(IndexType::Entity.code(), "entity");
        assert_eq!(IndexType::KnowledgeGraph.code(), "kg");
    }
}
