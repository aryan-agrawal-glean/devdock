//! Core types and traits for DevDock.
//!
//! This crate provides the foundational types used throughout DevDock,
//! including profiles, services, infrastructure components, and errors.

pub mod error;
pub mod profile;
pub mod service;
pub mod types;
pub mod update;

pub use error::{Error, Result};
pub use profile::{InfraConfig, IniOverride, Profile, ServiceConfig};
pub use service::{RedeployMetadata, ServiceIdentityMetadata, ServiceStatusMetadata};
pub use update::{
    IdentityValue, ServiceIdentity, StageResult, StageStatus, UpdateGeneration, UpdateReadiness,
    UpdateStage,
};
