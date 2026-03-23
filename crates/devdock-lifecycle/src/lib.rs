//! DevDock Lifecycle State Machine
//!
//! Provides an explicit state machine for the DevDock startup/shutdown lifecycle.
//! The lifecycle transitions through well-defined states, firing plugin hooks
//! at each transition point.

pub mod context;
pub mod hooks;
pub mod machine;
pub mod state;

pub use context::{
    ConfigContext, ErrorContext, InfraContext, ProfileContext, ReadyContext, ResourceContext,
    StopContext, TiltContext,
};
pub use hooks::{Hook, HookRunner, ResourceKind};
pub use machine::{LifecycleConfig, LifecycleMachine, LifecycleOutput};
pub use state::State;
