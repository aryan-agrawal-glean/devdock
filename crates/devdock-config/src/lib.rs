//! Configuration management for DevDock.
//!
//! This crate handles loading, parsing, and validating DevDock profiles.

pub mod dependency;
pub mod loader;
pub mod validator;
pub mod service_config;
pub mod dependency_graph;
pub mod platform;
pub mod ini_generator;
pub mod profile_generator;
pub mod bazel;
pub mod generator;
pub mod env_expansion;
pub mod ini_parser;
pub mod gcs_storage;
pub mod constants;
pub mod sql;
pub mod opensearch;
pub mod sql_import;
pub mod opensearch_import;
pub mod docker;
pub mod watch_patterns;

pub use dependency::DependencyResolver;
pub use loader::ProfileLoader;
pub use validator::ProfileValidator;
pub use service_config::{ServiceConfig, ServiceType};
pub use dependency_graph::{DependencyGraph, ServiceNode};
pub use platform::{Platform, IniConfig, TempProfile, create_platform};
pub use ini_generator::IniGenerator;
pub use profile_generator::TempProfileGenerator;
pub use bazel::BazelQueryEngine;
pub use generator::{ConfigGenerator, WatchStrategy};
pub use env_expansion::expand_env_vars;
pub use ini_parser::{ParsedIniConfig, parse_db_tables_query, parse_db_specs};
pub use gcs_storage::{GcsStorage, DynamicIniLoader, SqlDumpFetcher};

// New modules for standalone operation (replacing python_scio dependencies)
pub use constants::{
    // OpenSearch constants
    ELASTICSEARCH_NAMESPACES, DEFAULT_NAMESPACE,
    // SQL constants
    FRONTEND_INSTANCE, BACKEND_INSTANCE, PORT_START, IAM_PORT_START,
    // IAM constants
    COMPUTE_ENGINE_SERVICE_ACCOUNT,
    // Utility functions
    instance_name_to_type, zone_to_region,
};
pub use sql::{SqlUtils, get_bastion_port_map, get_frontend_databases, get_backend_databases};
pub use opensearch::{
    get_elastic_namespace, get_elastic_node_pool, OpenSearchConfig, IndexType,
};
pub use sql_import::{SqlOperationsManager, GoogleSqlManager, AwsSqlManager};
pub use opensearch_import::OpenSearchCopier;
pub use watch_patterns::{WatchPatternGenerator, WatchPatternConfig, ServiceType as WatchServiceType, HybridWatchStrategy};

