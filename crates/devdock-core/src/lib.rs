//! Core types and traits for DevDock.
//!
//! This crate provides the foundational types used throughout DevDock,
//! including profiles, services, infrastructure components, and errors.

pub mod error;
pub mod profile;
pub mod service;
pub mod types;

pub use error::{Error, Result};

