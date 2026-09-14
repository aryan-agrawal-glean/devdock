//! Configuration management for DevDock.
//!
//! This crate handles loading, parsing, and validating DevDock profiles.

pub mod bazel;
pub mod constants;
pub mod dependency;
pub mod dependency_graph;
pub mod docker;
pub mod env_expansion;
pub mod gcs_storage;
pub mod generator;
pub mod ini_generator;
pub mod ini_parser;
pub mod loader;
pub mod opensearch;
pub mod opensearch_import;
pub mod platform;
pub mod profile_generator;
pub mod s3_storage;
pub mod service_config;
pub mod sql;
pub mod sql_import;
pub mod sql_policy;
pub mod validator;
pub mod watch_patterns;

pub use bazel::BazelQueryEngine;
pub use dependency::DependencyResolver;
pub use dependency_graph::{DependencyGraph, ServiceNode};
pub use env_expansion::{expand_env_vars, resolve_identity, IdentityResolutionError};
pub use gcs_storage::{DynamicIniLoader, GcsStorage};
pub use generator::{ConfigGenerator, WatchStrategy};
pub use ini_generator::IniGenerator;
pub use ini_parser::{parse_db_specs, parse_db_tables_query, ParsedIniConfig};
pub use loader::{resolve_profile, ProfileLoader};
pub use platform::{create_platform, IniConfig, Platform, TempProfile};
pub use profile_generator::TempProfileGenerator;
pub use s3_storage::{S3DynamicIniLoader, S3Storage};
pub use service_config::{ServiceConfig, ServiceType};
pub use validator::ProfileValidator;

// New modules for standalone operation (replacing python_scio dependencies)
pub use constants::{
    // Utility functions
    instance_name_to_type,
    zone_to_region,
    BACKEND_INSTANCE,
    // IAM constants
    COMPUTE_ENGINE_SERVICE_ACCOUNT,
    DEFAULT_NAMESPACE,
    // OpenSearch constants
    ELASTICSEARCH_NAMESPACES,
    // SQL constants
    FRONTEND_INSTANCE,
    IAM_PORT_START,
    PORT_START,
};
pub use opensearch::{get_elastic_namespace, get_elastic_node_pool, IndexType, OpenSearchConfig};
pub use opensearch_import::OpenSearchCopier;
pub use sql::{get_backend_databases, get_bastion_port_map, get_frontend_databases, SqlUtils};
pub use sql_import::{AwsSqlManager, FastExportConfig, GoogleSqlManager, SqlOperationsManager};
pub use sql_policy::{
    is_keyval_identity, normalize_table_identities, resolve_table_copy_plan, KeyValPolicy,
    TableCopyPlan, KEYVAL_TABLE_IDENTITY,
};
pub use watch_patterns::{
    HybridWatchStrategy, ServiceType as WatchServiceType, WatchPatternConfig, WatchPatternGenerator,
};
