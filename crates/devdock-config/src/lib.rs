//! Configuration management for DevDock.
//!
//! This crate handles loading, parsing, and validating DevDock profiles.

pub mod dependency;
pub mod loader;
pub mod validator;

pub use dependency::DependencyResolver;
pub use loader::ProfileLoader;
pub use validator::ProfileValidator;

