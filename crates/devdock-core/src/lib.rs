//! Core types and traits for DevDock.
//!
//! This crate provides the foundational types used throughout DevDock,
//! including profiles, services, infrastructure components, and errors.

pub mod error;
pub mod plugin;
pub mod plugin_manager;
pub mod profile;
pub mod project_config;
pub mod resolver;
pub mod service;
pub mod service_config;
pub mod tiltfile_generator;
pub mod types;

pub use error::{Error, Result};
pub use plugin::BackendRegistry;
pub use plugin_manager::PluginManager;
pub use profile::{InfraConfig, IniOverride, Profile, ServiceConfig};
pub use project_config::{
    PluginHooks, PluginsSection, ProjectConfig, RemoteSection, RemoteSyncFile, ResourceHooks,
};
pub use resolver::{resolve, ResolvedDeps};
pub use service_config::{discover_services, ComponentKind, DevdockServiceConfig};
