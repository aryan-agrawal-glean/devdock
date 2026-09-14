//! Additive update-generation and readiness primitives.
//!
//! These types describe the contract a later Scio transaction can use. They
//! do not perform rollback or external actions; incomplete or failed stages
//! remain non-ready so callers cannot mistake a prior healthy state for a new
//! successful update.

use serde::{Deserialize, Serialize};

/// Ordered stages of a profile/configuration update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStage {
    Resolve,
    Plan,
    Quiesce,
    Prepare,
    Hydrate,
    DatabaseAssert,
    ConfigPromote,
    Build,
    Stage,
    Deploy,
    LoadedIdentity,
    ApplicationReady,
}

impl UpdateStage {
    pub const ORDERED: [Self; 12] = [
        Self::Resolve,
        Self::Plan,
        Self::Quiesce,
        Self::Prepare,
        Self::Hydrate,
        Self::DatabaseAssert,
        Self::ConfigPromote,
        Self::Build,
        Self::Stage,
        Self::Deploy,
        Self::LoadedIdentity,
        Self::ApplicationReady,
    ];
}

/// Fail-closed status for one stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    Pending,
    Succeeded,
    Failed,
    NonReady,
    Unknown,
}

/// Non-secret identity values carried by optional status/redeploy metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityValue {
    Known(String),
    Unknown,
}

/// Expected and observed identities for one service generation.
///
/// Every field is optional for compatibility with old metadata. Consumers
/// must use [`ServiceIdentity::value`] when they need an explicit unknown
/// result; they must not synthesize a digest or artifact identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded: Option<String>,
}

impl ServiceIdentity {
    pub fn value(&self, stage: UpdateStage) -> IdentityValue {
        let value = match stage {
            UpdateStage::Resolve | UpdateStage::Plan | UpdateStage::Prepare => &self.expected,
            UpdateStage::Build => &self.built,
            UpdateStage::Stage => &self.staged,
            UpdateStage::Deploy => &self.deployed,
            UpdateStage::LoadedIdentity | UpdateStage::ApplicationReady => &self.loaded,
            UpdateStage::Quiesce
            | UpdateStage::Hydrate
            | UpdateStage::DatabaseAssert
            | UpdateStage::ConfigPromote => &self.expected,
        };
        value.clone().map_or(IdentityValue::Unknown, IdentityValue::Known)
    }

    pub fn matches_expected(&self, observed: UpdateStage) -> bool {
        match (&self.expected, self.value(observed)) {
            (Some(expected), IdentityValue::Known(actual)) => expected == &actual,
            _ => false,
        }
    }
}

/// One stage's result, including a stage-specific failure without exposing
/// unrelated or secret-bearing diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageResult {
    pub stage: UpdateStage,
    pub status: StageStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Readiness of an update generation. Unknown is deliberately not ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateReadiness {
    Ready,
    NonReady,
    Unknown,
}

/// A versioned update generation with fail-closed stage semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateGeneration {
    pub generation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tilt_session: Option<String>,
    #[serde(default)]
    pub stages: Vec<StageResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<ServiceIdentity>,
}

impl UpdateGeneration {
    pub fn new(generation_id: impl Into<String>) -> Self {
        Self {
            generation_id: generation_id.into(),
            tilt_session: None,
            stages: Vec::new(),
            identity: None,
        }
    }

    pub fn with_tilt_session(mut self, tilt_session: impl Into<String>) -> Self {
        self.tilt_session = Some(tilt_session.into());
        self
    }

    pub fn record(&mut self, stage: UpdateStage, status: StageStatus) {
        self.record_with_error(stage, status, None);
    }

    pub fn record_with_error(
        &mut self,
        stage: UpdateStage,
        status: StageStatus,
        error: Option<String>,
    ) {
        if let Some(existing) = self.stages.iter_mut().find(|result| result.stage == stage) {
            *existing = StageResult { stage, status, error };
        } else {
            self.stages.push(StageResult { stage, status, error });
            self.stages.sort_by_key(|result| {
                UpdateStage::ORDERED
                    .iter()
                    .position(|candidate| candidate == &result.stage)
                    .unwrap_or(UpdateStage::ORDERED.len())
            });
        }
    }

    pub fn fail(&mut self, stage: UpdateStage, error: impl Into<String>) {
        self.record_with_error(stage, StageStatus::Failed, Some(error.into()));
    }

    pub fn stage_status(&self, stage: UpdateStage) -> StageStatus {
        self.stages
            .iter()
            .find(|result| result.stage == stage)
            .map_or(StageStatus::Unknown, |result| result.status)
    }

    /// Return readiness only when every stage through application readiness
    /// was explicitly successful. Any missing/unknown stage is not a pass.
    pub fn readiness(&self) -> UpdateReadiness {
        if self
            .stages
            .iter()
            .any(|result| matches!(result.status, StageStatus::Failed | StageStatus::NonReady))
        {
            return UpdateReadiness::NonReady;
        }

        let complete = UpdateStage::ORDERED
            .iter()
            .all(|stage| self.stage_status(*stage) == StageStatus::Succeeded);
        if complete {
            UpdateReadiness::Ready
        } else {
            UpdateReadiness::Unknown
        }
    }

    pub fn is_ready(&self) -> bool {
        self.readiness() == UpdateReadiness::Ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_generation() -> UpdateGeneration {
        let mut generation = UpdateGeneration::new("generation-2").with_tilt_session("tilt-2");
        for stage in UpdateStage::ORDERED {
            generation.record(stage, StageStatus::Succeeded);
        }
        generation
    }

    #[test]
    fn missing_new_metadata_is_unknown_and_not_ready() {
        let generation = UpdateGeneration::new("generation-1");
        assert_eq!(generation.readiness(), UpdateReadiness::Unknown);
        assert!(!generation.is_ready());
        assert_eq!(generation.stage_status(UpdateStage::Build), StageStatus::Unknown);
        assert_eq!(
            ServiceIdentity::default().value(UpdateStage::LoadedIdentity),
            IdentityValue::Unknown
        );
    }

    #[test]
    fn stage_failure_is_specific_and_fail_closed() {
        let mut generation = complete_generation();
        generation.fail(UpdateStage::Hydrate, "sql hydration failed");
        assert_eq!(generation.stage_status(UpdateStage::Hydrate), StageStatus::Failed);
        assert_eq!(generation.readiness(), UpdateReadiness::NonReady);
        assert!(!generation.is_ready());
    }

    #[test]
    fn readiness_requires_every_stage_of_this_generation() {
        let generation = complete_generation();
        assert!(generation.is_ready());
        assert_eq!(generation.tilt_session.as_deref(), Some("tilt-2"));
    }
}
