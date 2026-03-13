//! Configuration management for DevDock.
//!
//! This crate handles loading, parsing, and validating DevDock profiles.
//!
//! ## Module Organization
//!
//! **Core modules** (generic, no cloud dependencies):
//! - `loader` — Profile file discovery and loading
//! - `validator` — Profile validation
//! - `dependency` — Dependency resolution
//! - `dependency_graph` — Dependency graph builder
//! - `watch_patterns` — File watch pattern generation
//! - `bazel` — Bazel query engine
//! - `docker` — Docker utilities
//! - `env_expansion` — Environment variable expansion
//! - `service_config` — Service configuration types
//!
//! **Cloud modules** (behind `cloud` feature flag, used by config-generator plugin):
//! - `generator` — Main config orchestrator
//! - `ini_generator` — devdock.ini generation
//! - `ini_parser` — INI parsing utilities
//! - `profile_generator` — temp_profile.yaml generation
//! - `platform` — Cloud platform abstraction (GKE/EKS/AKS)
//! - `constants` — Cloud-specific constants
//! - `gcs_storage` / `s3_storage` — Cloud storage
//! - `sql` / `sql_import` — SQL hydration
//! - `opensearch` / `opensearch_import` — OpenSearch management

// ─── Core modules (always available) ────────────────────────────────────────

pub mod bazel;
pub mod dependency;
pub mod dependency_graph;
pub mod docker;
pub mod env_expansion;
pub mod loader;
pub mod service_config;
pub mod validator;
pub mod watch_patterns;

pub use bazel::BazelQueryEngine;
pub use dependency::DependencyResolver;
pub use dependency_graph::{DependencyGraph, ServiceNode};
pub use env_expansion::expand_env_vars;
pub use loader::ProfileLoader;
pub use service_config::{ServiceConfig, ServiceType};
pub use validator::ProfileValidator;
pub use watch_patterns::{
    HybridWatchStrategy, ServiceType as WatchServiceType, WatchPatternConfig, WatchPatternGenerator,
};

// ─── Cloud modules (behind feature flag) ────────────────────────────────────

#[cfg(feature = "cloud")]
pub mod constants;
#[cfg(feature = "cloud")]
pub mod gcs_storage;
#[cfg(feature = "cloud")]
pub mod generator;
#[cfg(feature = "cloud")]
pub mod ini_generator;
#[cfg(feature = "cloud")]
pub mod ini_parser;
#[cfg(feature = "cloud")]
pub mod opensearch;
#[cfg(feature = "cloud")]
pub mod opensearch_import;
#[cfg(feature = "cloud")]
pub mod platform;
#[cfg(feature = "cloud")]
pub mod profile_generator;
#[cfg(feature = "cloud")]
pub mod s3_storage;
#[cfg(feature = "cloud")]
pub mod sql;
#[cfg(feature = "cloud")]
pub mod sql_import;

#[cfg(feature = "cloud")]
pub use gcs_storage::{DynamicIniLoader, GcsStorage};
#[cfg(feature = "cloud")]
pub use generator::{ConfigGenerator, WatchStrategy};
#[cfg(feature = "cloud")]
pub use ini_generator::IniGenerator;
#[cfg(feature = "cloud")]
pub use ini_parser::{parse_db_specs, parse_db_tables_query, ParsedIniConfig};
#[cfg(feature = "cloud")]
pub use platform::{create_platform, IniConfig, Platform, TempProfile};
#[cfg(feature = "cloud")]
pub use profile_generator::TempProfileGenerator;
#[cfg(feature = "cloud")]
pub use s3_storage::{S3DynamicIniLoader, S3Storage};

#[cfg(feature = "cloud")]
pub use constants::{
    instance_name_to_type, zone_to_region, BACKEND_INSTANCE, COMPUTE_ENGINE_SERVICE_ACCOUNT,
    DEFAULT_NAMESPACE, ELASTICSEARCH_NAMESPACES, FRONTEND_INSTANCE, IAM_PORT_START, PORT_START,
};
#[cfg(feature = "cloud")]
pub use opensearch::{get_elastic_namespace, get_elastic_node_pool, IndexType, OpenSearchConfig};
#[cfg(feature = "cloud")]
pub use opensearch_import::OpenSearchCopier;
#[cfg(feature = "cloud")]
pub use sql::{get_backend_databases, get_bastion_port_map, get_frontend_databases, SqlUtils};
#[cfg(feature = "cloud")]
pub use sql_import::{AwsSqlManager, FastExportConfig, GoogleSqlManager, SqlOperationsManager};
