//! Lifecycle state enum and transition logic.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The lifecycle states of a DevDock session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Parsing CLI args, initial setup.
    Initializing,
    /// Profile YAML loaded and validated.
    ProfileLoaded,
    /// All plugins spawned and initialized.
    PluginsReady,
    /// Docker checked, networks created, ports allocated.
    InfraReady,
    /// Config files generated (temp_profile.yaml, etc.).
    ConfigReady,
    /// Tiltfile generated, ready to launch Tilt.
    TiltfileReady,
    /// Tilt process spawned.
    TiltRunning,
    /// Per-resource startup in progress.
    ResourcesStarting,
    /// All resources healthy, steady state.
    Running,
    /// Shutdown initiated.
    Stopping,
    /// Shutdown complete.
    Stopped,
    /// Unrecoverable error occurred.
    Failed,
}

impl State {
    /// Returns the next state in the normal (happy-path) lifecycle.
    pub fn next(&self) -> Option<State> {
        match self {
            Self::Initializing => Some(Self::ProfileLoaded),
            Self::ProfileLoaded => Some(Self::PluginsReady),
            Self::PluginsReady => Some(Self::InfraReady),
            Self::InfraReady => Some(Self::ConfigReady),
            Self::ConfigReady => Some(Self::TiltfileReady),
            Self::TiltfileReady => Some(Self::TiltRunning),
            Self::TiltRunning => Some(Self::ResourcesStarting),
            Self::ResourcesStarting => Some(Self::Running),
            Self::Running => Some(Self::Stopping),
            Self::Stopping => Some(Self::Stopped),
            Self::Stopped | Self::Failed => None,
        }
    }

    /// Returns true if this is a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Stopped | Self::Failed)
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Initializing => write!(f, "Initializing"),
            Self::ProfileLoaded => write!(f, "ProfileLoaded"),
            Self::PluginsReady => write!(f, "PluginsReady"),
            Self::InfraReady => write!(f, "InfraReady"),
            Self::ConfigReady => write!(f, "ConfigReady"),
            Self::TiltfileReady => write!(f, "TiltfileReady"),
            Self::TiltRunning => write!(f, "TiltRunning"),
            Self::ResourcesStarting => write!(f, "ResourcesStarting"),
            Self::Running => write!(f, "Running"),
            Self::Stopping => write!(f, "Stopping"),
            Self::Stopped => write!(f, "Stopped"),
            Self::Failed => write!(f, "Failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_transitions() {
        let mut state = State::Initializing;
        let expected = vec![
            State::ProfileLoaded,
            State::PluginsReady,
            State::InfraReady,
            State::ConfigReady,
            State::TiltfileReady,
            State::TiltRunning,
            State::ResourcesStarting,
            State::Running,
            State::Stopping,
            State::Stopped,
        ];
        for expected_state in expected {
            state = state.next().unwrap();
            assert_eq!(state, expected_state);
        }
        assert!(state.next().is_none());
    }

    #[test]
    fn terminal_states() {
        assert!(State::Stopped.is_terminal());
        assert!(State::Failed.is_terminal());
        assert!(!State::Running.is_terminal());
    }

    #[test]
    fn display() {
        assert_eq!(State::Initializing.to_string(), "Initializing");
        assert_eq!(State::Running.to_string(), "Running");
    }

    #[test]
    fn serialize() {
        let json = serde_json::to_string(&State::TiltRunning).unwrap();
        assert_eq!(json, "\"tilt_running\"");
    }
}
