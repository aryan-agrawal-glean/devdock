//! Constants for DevDock configuration.
//!
//! These constants replace the python_scio dependencies:
//! - python_scio.core.elastic_common (namespace constants)
//! - python_scio.core.sql_constants (SQL instance constants, port mapping)
//! - python_scio.core.config_constants (IndexingConfig)
//! - python_scio.deployment.common.iam_lib (service account constants)

// =============================================================================
// OpenSearch/Elasticsearch Namespace Constants
// =============================================================================

/// Elasticsearch namespace constants (from python_scio.core.elastic_common)
pub const ELASTICSEARCH_1_NAMESPACE: &str = "elasticsearch-1-namespace";
pub const ELASTICSEARCH_2_NAMESPACE: &str = "elasticsearch-2-namespace";
pub const ELASTICSEARCH_3_NAMESPACE: &str = "elasticsearch-3-namespace";
pub const ELASTICSEARCH_4_NAMESPACE: &str = "elasticsearch-4-namespace";
pub const ELASTICSEARCH_5_NAMESPACE: &str = "elasticsearch-5-namespace";
pub const ELASTICSEARCH_6_NAMESPACE: &str = "elasticsearch-6-namespace";
pub const ELASTICSEARCH_7_NAMESPACE: &str = "elasticsearch-7-namespace";
pub const ELASTICSEARCH_8_NAMESPACE: &str = "elasticsearch-8-namespace";
pub const ELASTICSEARCH_9_NAMESPACE: &str = "elasticsearch-9-namespace";
pub const ELASTICSEARCH_10_NAMESPACE: &str = "elasticsearch-10-namespace";
pub const ELASTICSEARCH_11_NAMESPACE: &str = "elasticsearch-11-namespace";
pub const ELASTICSEARCH_12_NAMESPACE: &str = "elasticsearch-12-namespace";

/// All valid Elasticsearch namespaces
pub const ELASTICSEARCH_NAMESPACES: &[&str] = &[
    ELASTICSEARCH_1_NAMESPACE,
    ELASTICSEARCH_2_NAMESPACE,
    ELASTICSEARCH_3_NAMESPACE,
    ELASTICSEARCH_4_NAMESPACE,
    ELASTICSEARCH_5_NAMESPACE,
    ELASTICSEARCH_6_NAMESPACE,
    ELASTICSEARCH_7_NAMESPACE,
    ELASTICSEARCH_8_NAMESPACE,
    ELASTICSEARCH_9_NAMESPACE,
    ELASTICSEARCH_10_NAMESPACE,
    ELASTICSEARCH_11_NAMESPACE,
    ELASTICSEARCH_12_NAMESPACE,
];

/// Default Kubernetes namespace
pub const DEFAULT_NAMESPACE: &str = "default";

/// Experimental node pool namespaces
pub const ELASTIC_EXPERIMENTAL_NODE_POOL_1: &str = "elastic-exp-node-pool-1";
pub const ELASTIC_EXPERIMENTAL_NAMESPACE_1: &str = "elastic-exp-namespace-1";
pub const ELASTIC_EXPERIMENTAL_NODE_POOL_2: &str = "elastic-exp-node-pool-2";
pub const ELASTIC_EXPERIMENTAL_NAMESPACE_2: &str = "elastic-exp-namespace-2";

// =============================================================================
// SQL Instance Constants
// =============================================================================

/// Frontend SQL instance type (from python_scio.core.sql_constants)
pub const FRONTEND_INSTANCE: &str = "frontendInstance";

/// Backend SQL instance type
pub const BACKEND_INSTANCE: &str = "backendInstance";

/// Sharded SQL instance info prefix
pub const SHARDED_SQL_INSTANCE_INFO_PREFIX: &str = "documentDB";

/// Port range start for bastion connections (non-IAM)
pub const PORT_START: u16 = 3311;

/// Port range start for IAM-authenticated bastion connections
pub const IAM_PORT_START: u16 = 13311;

/// Environment variable to check for SQL public IP usage
pub const USE_SQL_PUBLIC_IP_ENV_VAR: &str = "USE_SQL_PUBLIC_IP";

// =============================================================================
// IAM Constants
// =============================================================================

/// Default compute engine service account (from python_scio.deployment.common.iam_lib)
pub const COMPUTE_ENGINE_SERVICE_ACCOUNT: &str = "compute-engine";

// =============================================================================
// Indexing Configuration Keys
// =============================================================================

/// Indexing configuration keys (from python_scio.core.config_constants.IndexingConfig)
pub struct IndexingConfig;

impl IndexingConfig {
    /// Config key for node pool name
    pub const NODE_POOL_NAME: &'static str = "index.nodePool";

    /// Config key for staging node pool name
    pub const STAGING_NODE_POOL_NAME: &'static str = "staging.index.nodePool";

    /// Config key for server URL
    pub const SERVER_URL: &'static str = "index.serverUrl";
}

// =============================================================================
// SQL Instance Type Mapping
// =============================================================================

/// Map SQL instance names to short type codes (from python_scio.platform.interfaces.sql_importer)
pub fn instance_name_to_type(instance_name: &str) -> &'static str {
    if instance_name.starts_with(FRONTEND_INSTANCE) {
        "FE"
    } else if instance_name.starts_with(BACKEND_INSTANCE) {
        "BE"
    } else if instance_name.starts_with(SHARDED_SQL_INSTANCE_INFO_PREFIX) {
        "DOC"
    } else {
        "UNKNOWN"
    }
}

// =============================================================================
// GCP Region/Zone Mapping
// =============================================================================

/// Get zones for a region
pub fn get_zones_for_region(region: &str) -> &'static [&'static str] {
    match region {
        "us-central1" => &["us-central1-a", "us-central1-b", "us-central1-c", "us-central1-f"],
        "us-east1" => &["us-east1-b", "us-east1-c", "us-east1-d"],
        "us-east4" => &["us-east4-a", "us-east4-b", "us-east4-c"],
        "us-west1" => &["us-west1-a", "us-west1-b", "us-west1-c"],
        "us-west2" => &["us-west2-a", "us-west2-b", "us-west2-c"],
        "europe-west1" => &["europe-west1-b", "europe-west1-c", "europe-west1-d"],
        "europe-west2" => &["europe-west2-a", "europe-west2-b", "europe-west2-c"],
        "asia-east1" => &["asia-east1-a", "asia-east1-b", "asia-east1-c"],
        "asia-northeast1" => &["asia-northeast1-a", "asia-northeast1-b", "asia-northeast1-c"],
        "australia-southeast1" => &["australia-southeast1-a", "australia-southeast1-b", "australia-southeast1-c"],
        _ => &[],
    }
}

/// Extract region from zone (e.g., "us-central1-a" -> "us-central1")
pub fn zone_to_region(zone: &str) -> &str {
    if zone.len() > 2 {
        &zone[..zone.len() - 2]
    } else {
        zone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_elasticsearch_namespaces() {
        assert_eq!(ELASTICSEARCH_NAMESPACES.len(), 12);
        assert_eq!(ELASTICSEARCH_NAMESPACES[0], "elasticsearch-1-namespace");
        assert_eq!(ELASTICSEARCH_NAMESPACES[11], "elasticsearch-12-namespace");
    }

    #[test]
    fn test_instance_name_to_type() {
        assert_eq!(instance_name_to_type("frontendInstance"), "FE");
        assert_eq!(instance_name_to_type("frontendInstance2"), "FE");
        assert_eq!(instance_name_to_type("backendInstance"), "BE");
        assert_eq!(instance_name_to_type("backendInstance3"), "BE");
        assert_eq!(instance_name_to_type("documentDB_0"), "DOC");
        assert_eq!(instance_name_to_type("unknown"), "UNKNOWN");
    }

    #[test]
    fn test_zone_to_region() {
        assert_eq!(zone_to_region("us-central1-a"), "us-central1");
        assert_eq!(zone_to_region("europe-west1-b"), "europe-west1");
        assert_eq!(zone_to_region("asia-northeast1-c"), "asia-northeast1");
    }

    #[test]
    fn test_zones_for_region() {
        let zones = get_zones_for_region("us-central1");
        assert_eq!(zones.len(), 4);
        assert!(zones.contains(&"us-central1-a"));
        assert!(zones.contains(&"us-central1-c"));
    }
}

