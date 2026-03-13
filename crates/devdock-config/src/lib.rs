//! Configuration management for DevDock.
//!
//! This crate handles loading, parsing, and validating DevDock profiles.
//!
//! ## Module Organization
//!
//! - `loader` — Profile file discovery and loading
//! - `validator` — Profile validation
//! - `dependency` — Dependency resolution
//! - `dependency_graph` — Dependency graph builder
//! - `watch_patterns` — File watch pattern generation
//! - `bazel` — Bazel query engine
//! - `docker` — Docker utilities
//! - `env_expansion` — Environment variable expansion
//! - `service_config` — Service configuration types

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
