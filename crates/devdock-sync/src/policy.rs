//! Versioned, repository-relative policy for incremental synchronization.
//!
//! Repository policy bytes are deliberately supplied by the caller. This module never reads the
//! working-tree policy file, which lets the E2E coordinator resolve trusted bytes from the agreed
//! Git anchor without allowing a working-tree edit to bless itself.

use glob::Pattern;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Component, Path};
use std::sync::OnceLock;
use thiserror::Error;

pub const SYNC_POLICY_SCHEMA_VERSION: u32 = 1;
pub const UNIVERSAL_BASELINE_VERSION: &str = "devdock-sync/v1";

static UNIVERSAL_MATCHER: OnceLock<SyncMatcher> = OnceLock::new();

const MAX_POLICY_BYTES: usize = 64 * 1024;
const MAX_LIST_ITEMS: usize = 256;
const MAX_PATTERN_BYTES: usize = 256;

const BASELINE_MONITORED_ROOTS: &[&str] = &[
    "data",
    "go",
    "python_scio",
    "typescript",
    "java",
    "com/askscio/proto",
    "config",
    "javatests",
    "openapi",
    "devdock",
];

const BASELINE_SYNC_EXTENSIONS: &[&str] = &[
    ".bazel",
    ".go",
    ".py",
    ".ts",
    ".tsx",
    ".java",
    ".proto",
    ".ini",
    ".json",
    ".yaml",
    ".yml",
    ".sh",
    ".sql",
    ".gradle",
    ".properties",
    ".csv",
];

const BASELINE_SYNC_FILENAMES: &[&str] = &["pom.xml", "Makefile", "package.json", "pnpm-lock.yaml"];

const BASELINE_SYNC_WITHOUT_TRACKING: &[&str] = &["config/custom.ini", "devdock/custom.ini"];

const BASELINE_SYNC_WITHOUT_TRACKING_PREFIXES: &[&str] = &[
    "devdock/profiles/user_profiles",
    "devdock/profiles/integration_tests",
    "devdock/profiles/user_profile.yaml",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenignRuleV1 {
    pub rule_id: String,
    pub glob: String,
}

/// Bounded repository-owned additions to DevDock's universal baseline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositorySyncPolicyV1 {
    pub schema_version: u32,
    pub compatible_since_sha: String,
    #[serde(default)]
    pub monitored_roots: Vec<String>,
    #[serde(default)]
    pub sync_extensions: Vec<String>,
    #[serde(default)]
    pub sync_filenames: Vec<String>,
    #[serde(default)]
    pub impact_roots: Vec<String>,
    #[serde(default)]
    pub benign_out_of_scope: Vec<BenignRuleV1>,
}

impl RepositorySyncPolicyV1 {
    /// Parse and validate an exact policy snapshot obtained from a trusted Git object.
    pub fn from_snapshot(bytes: &[u8]) -> Result<Self, PolicyError> {
        if bytes.len() > MAX_POLICY_BYTES {
            return Err(PolicyError::TooLarge { actual: bytes.len(), max: MAX_POLICY_BYTES });
        }

        let policy: Self = serde_json::from_slice(bytes)
            .map_err(|error| PolicyError::InvalidJson(error.to_string()))?;
        policy.validate()?;
        Ok(policy)
    }

    fn validate(&self) -> Result<(), PolicyError> {
        if self.schema_version != SYNC_POLICY_SCHEMA_VERSION {
            return Err(PolicyError::UnsupportedSchema(self.schema_version));
        }

        let sha = &self.compatible_since_sha;
        if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(PolicyError::InvalidCompatibleSinceSha);
        }

        validate_count("monitored_roots", self.monitored_roots.len())?;
        validate_count("sync_extensions", self.sync_extensions.len())?;
        validate_count("sync_filenames", self.sync_filenames.len())?;
        validate_count("impact_roots", self.impact_roots.len())?;
        validate_count("benign_out_of_scope", self.benign_out_of_scope.len())?;

        for root in self.monitored_roots.iter().chain(&self.impact_roots) {
            validate_repo_pattern(root, true)?;
        }
        for extension in &self.sync_extensions {
            if extension.len() > MAX_PATTERN_BYTES
                || !extension.starts_with('.')
                || extension.len() == 1
                || extension.contains(['/', '\\'])
            {
                return Err(PolicyError::InvalidExtension(extension.clone()));
            }
        }
        for filename in &self.sync_filenames {
            if filename.is_empty()
                || filename.len() > MAX_PATTERN_BYTES
                || filename.contains(['/', '\\'])
                || matches!(filename.as_str(), "." | "..")
            {
                return Err(PolicyError::InvalidFilename(filename.clone()));
            }
        }

        let mut rule_ids = HashSet::new();
        for rule in &self.benign_out_of_scope {
            if rule.rule_id.is_empty()
                || rule.rule_id.len() > 64
                || !rule.rule_id.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
                })
            {
                return Err(PolicyError::InvalidRuleId(rule.rule_id.clone()));
            }
            if !rule_ids.insert(&rule.rule_id) {
                return Err(PolicyError::DuplicateRuleId(rule.rule_id.clone()));
            }
            validate_repo_pattern(&rule.glob, false)?;
            Pattern::new(&rule.glob).map_err(|_| PolicyError::InvalidGlob(rule.glob.clone()))?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    Universal,
    Repository,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathClassification {
    Syncable,
    BenignOutOfScope { source: RuleSource, rule_id: String },
    ImpactUnknownOutOfScope,
}

#[derive(Debug, Clone)]
struct CompiledBenignRule {
    source: RuleSource,
    rule_id: String,
    pattern: Pattern,
}

/// The single matcher used by initial sync, daemon filtering, and E2E classification.
#[derive(Debug, Clone)]
pub struct SyncMatcher {
    monitored_roots: Vec<String>,
    sync_extensions: HashSet<String>,
    sync_filenames: HashSet<String>,
    impact_roots: Vec<String>,
    benign_rules: Vec<CompiledBenignRule>,
}

impl Default for SyncMatcher {
    fn default() -> Self {
        Self::universal()
    }
}

impl SyncMatcher {
    /// Conservative universal matcher. It preserves the existing normal-sync allowlist.
    pub fn universal() -> Self {
        let monitored_roots =
            BASELINE_MONITORED_ROOTS.iter().map(|value| (*value).into()).collect();
        let impact_roots = BASELINE_MONITORED_ROOTS.iter().map(|value| (*value).into()).collect();
        let benign_rules = [
            ("universal_editor_vscode", ".vscode/**"),
            ("universal_editor_idea", ".idea/**"),
            ("universal_markdown", "**/*.md"),
        ]
        .into_iter()
        .map(|(rule_id, glob)| CompiledBenignRule {
            source: RuleSource::Universal,
            rule_id: rule_id.to_string(),
            pattern: Pattern::new(glob).expect("built-in benign glob must be valid"),
        })
        .collect();

        Self {
            monitored_roots,
            sync_extensions: BASELINE_SYNC_EXTENSIONS.iter().map(|value| (*value).into()).collect(),
            sync_filenames: BASELINE_SYNC_FILENAMES.iter().map(|value| (*value).into()).collect(),
            impact_roots,
            benign_rules,
        }
    }

    /// Extend the universal matcher with a validated repository snapshot.
    pub fn with_repository_policy(policy: &RepositorySyncPolicyV1) -> Self {
        let mut matcher = Self::universal();
        extend_unique(&mut matcher.monitored_roots, &policy.monitored_roots);
        // A repository-monitored root is necessarily capable of affecting the environment even
        // when an individual path under it does not match a supported sync extension.
        extend_unique(&mut matcher.impact_roots, &policy.monitored_roots);
        extend_unique(&mut matcher.impact_roots, &policy.impact_roots);
        matcher.sync_extensions.extend(policy.sync_extensions.iter().cloned());
        matcher.sync_filenames.extend(policy.sync_filenames.iter().cloned());
        matcher.benign_rules.extend(policy.benign_out_of_scope.iter().map(|rule| {
            CompiledBenignRule {
                source: RuleSource::Repository,
                rule_id: rule.rule_id.clone(),
                pattern: Pattern::new(&rule.glob).expect("validated repository benign glob"),
            }
        }));
        matcher
    }

    pub fn monitored_roots(&self) -> impl Iterator<Item = &str> {
        self.monitored_roots.iter().map(String::as_str)
    }

    pub fn is_syncable(&self, path: &Path) -> bool {
        let Some(path) = normalized_relative_path(path) else {
            return false;
        };
        if !self.monitored_roots.iter().any(|root| path_is_within(&path, root)) {
            return false;
        }

        let path = Path::new(&path);
        if path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|filename| self.sync_filenames.contains(filename))
        {
            return true;
        }

        path.extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| self.sync_extensions.contains(&format!(".{extension}")))
    }

    pub fn syncs_without_git_tracking(&self, path: &Path) -> bool {
        let Some(path) = normalized_relative_path(path) else {
            return false;
        };
        BASELINE_SYNC_WITHOUT_TRACKING.contains(&path.as_str())
            || BASELINE_SYNC_WITHOUT_TRACKING_PREFIXES
                .iter()
                .any(|prefix| path_is_within(&path, prefix))
    }

    pub fn classify(&self, path: &Path) -> PathClassification {
        let Some(path) = normalized_relative_path(path) else {
            return PathClassification::ImpactUnknownOutOfScope;
        };
        if self.is_syncable(Path::new(&path)) {
            return PathClassification::Syncable;
        }

        let is_impact = self.impact_roots.iter().any(|root| path_is_within(&path, root));
        if !is_impact {
            if let Some(rule) =
                self.benign_rules.iter().find(|rule| glob_matches(&rule.pattern, &path))
            {
                return PathClassification::BenignOutOfScope {
                    source: rule.source,
                    rule_id: rule.rule_id.clone(),
                };
            }
        }

        PathClassification::ImpactUnknownOutOfScope
    }
}

/// Process-wide universal matcher used by normal startup and compatibility entry points.
pub fn universal_matcher() -> &'static SyncMatcher {
    UNIVERSAL_MATCHER.get_or_init(SyncMatcher::universal)
}

fn validate_count(field: &'static str, actual: usize) -> Result<(), PolicyError> {
    if actual > MAX_LIST_ITEMS {
        Err(PolicyError::TooManyItems { field, actual, max: MAX_LIST_ITEMS })
    } else {
        Ok(())
    }
}

fn validate_repo_pattern(value: &str, allow_dot: bool) -> Result<(), PolicyError> {
    if value.is_empty()
        || value.len() > MAX_PATTERN_BYTES
        || value.starts_with('/')
        || value.contains('\\')
        || value.contains('\0')
        || (!allow_dot && value == ".")
        || value.split('/').any(|component| component == "..")
    {
        return Err(PolicyError::InvalidRepositoryPattern(value.to_string()));
    }
    Ok(())
}

fn normalized_relative_path(path: &Path) -> Option<String> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return None;
    }

    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_str()?.to_string()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn path_is_within(path: &str, root: &str) -> bool {
    let root = root.trim_matches('/');
    root == "."
        || path == root
        || path.strip_prefix(root).is_some_and(|suffix| suffix.starts_with('/'))
}

fn glob_matches(pattern: &Pattern, path: &str) -> bool {
    pattern.matches(path)
        || pattern
            .as_str()
            .strip_prefix("**/")
            .is_some_and(|root_pattern| Pattern::new(root_pattern).is_ok_and(|p| p.matches(path)))
}

fn extend_unique(target: &mut Vec<String>, values: &[String]) {
    for value in values {
        let value = value.trim_matches('/').to_string();
        if !target.contains(&value) {
            target.push(value);
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PolicyError {
    #[error("sync policy is too large ({actual} bytes; maximum {max})")]
    TooLarge { actual: usize, max: usize },
    #[error("invalid sync policy JSON: {0}")]
    InvalidJson(String),
    #[error("unsupported sync policy schema version {0}")]
    UnsupportedSchema(u32),
    #[error("compatible_since_sha must be a full hexadecimal Git object ID")]
    InvalidCompatibleSinceSha,
    #[error("sync policy field {field} has {actual} entries; maximum is {max}")]
    TooManyItems { field: &'static str, actual: usize, max: usize },
    #[error("invalid repository-relative policy pattern: {0}")]
    InvalidRepositoryPattern(String),
    #[error("invalid sync extension: {0}")]
    InvalidExtension(String),
    #[error("invalid exact sync filename: {0}")]
    InvalidFilename(String),
    #[error("invalid benign rule id: {0}")]
    InvalidRuleId(String),
    #[error("duplicate benign rule id: {0}")]
    DuplicateRuleId(String),
    #[error("invalid benign glob: {0}")]
    InvalidGlob(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository_policy() -> RepositorySyncPolicyV1 {
        RepositorySyncPolicyV1 {
            schema_version: 1,
            compatible_since_sha: "a".repeat(40),
            monitored_roots: vec!["terraform".into(), ".".into()],
            sync_extensions: vec![".bzl".into(), ".tf".into()],
            sync_filenames: vec![
                "BUILD".into(),
                "BUILD.bazel".into(),
                "WORKSPACE".into(),
                "WORKSPACE.bazel".into(),
                "MODULE.bazel".into(),
                "Dockerfile".into(),
                "Tiltfile".into(),
            ],
            impact_roots: vec!["terraform".into()],
            benign_out_of_scope: vec![BenignRuleV1 {
                rule_id: "scio_scratch_notebooks".into(),
                glob: "scratch_notebooks/**".into(),
            }],
        }
    }

    #[test]
    fn universal_matcher_preserves_existing_allowlist() {
        let matcher = SyncMatcher::universal();
        assert!(matcher.is_syncable(Path::new("python_scio/service/main.py")));
        assert!(matcher.is_syncable(Path::new("java/service/BUILD.bazel")));
        assert!(matcher.is_syncable(Path::new("devdock/package.json")));
        assert!(!matcher.is_syncable(Path::new("python_scio/service/rules.bzl")));
        assert!(!matcher.is_syncable(Path::new("python_scio/README.md")));
        assert!(!matcher.is_syncable(Path::new("python_scio_evil/main.py")));
    }

    #[test]
    fn repository_policy_adds_build_and_terraform_inputs() {
        let matcher = SyncMatcher::with_repository_policy(&repository_policy());
        for path in [
            "python_scio/service/rules.bzl",
            "terraform/dev/main.tf",
            "java/service/BUILD",
            "BUILD.bazel",
            "WORKSPACE",
            "WORKSPACE.bazel",
            "MODULE.bazel",
            "docker/Dockerfile",
            "Tiltfile",
        ] {
            assert!(matcher.is_syncable(Path::new(path)), "expected {path} to be syncable");
        }
    }

    #[test]
    fn added_monitored_roots_are_also_impact_roots() {
        let mut policy = repository_policy();
        policy.monitored_roots.retain(|root| root != ".");
        let matcher = SyncMatcher::with_repository_policy(&policy);
        assert_eq!(
            matcher.classify(Path::new("terraform/README.md")),
            PathClassification::ImpactUnknownOutOfScope
        );
    }

    #[test]
    fn universal_benign_rules_do_not_override_impact_roots() {
        let matcher = SyncMatcher::universal();
        assert_eq!(
            matcher.classify(Path::new("README.md")),
            PathClassification::BenignOutOfScope {
                source: RuleSource::Universal,
                rule_id: "universal_markdown".into(),
            }
        );
        assert_eq!(
            matcher.classify(Path::new(".vscode/settings.json")),
            PathClassification::BenignOutOfScope {
                source: RuleSource::Universal,
                rule_id: "universal_editor_vscode".into(),
            }
        );
        assert_eq!(
            matcher.classify(Path::new("python_scio/README.md")),
            PathClassification::ImpactUnknownOutOfScope
        );
    }

    #[test]
    fn repository_benign_rule_records_its_source() {
        let mut policy = repository_policy();
        policy.monitored_roots.retain(|root| root != ".");
        let matcher = SyncMatcher::with_repository_policy(&policy);
        assert_eq!(
            matcher.classify(Path::new("scratch_notebooks/example.ipynb")),
            PathClassification::BenignOutOfScope {
                source: RuleSource::Repository,
                rule_id: "scio_scratch_notebooks".into(),
            }
        );
    }

    #[test]
    fn invalid_or_unsafe_paths_fail_closed() {
        let matcher = SyncMatcher::universal();
        assert!(!matcher.is_syncable(Path::new("../python_scio/main.py")));
        assert_eq!(
            matcher.classify(Path::new("../README.md")),
            PathClassification::ImpactUnknownOutOfScope
        );
    }

    #[test]
    fn exact_tracking_exceptions_are_centralized() {
        let matcher = SyncMatcher::universal();
        assert!(matcher.syncs_without_git_tracking(Path::new("config/custom.ini")));
        assert!(
            matcher.syncs_without_git_tracking(Path::new("devdock/profiles/user_profiles/me.yaml"))
        );
        assert!(!matcher.syncs_without_git_tracking(Path::new("config/secret.ini")));
    }

    #[test]
    fn parser_rejects_executable_or_network_metadata() {
        let bytes = br#"{
            "schema_version": 1,
            "compatible_since_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "monitored_roots": [],
            "sync_extensions": [],
            "sync_filenames": [],
            "impact_roots": [],
            "benign_out_of_scope": [],
            "command": ["sh", "-c", "anything"]
        }"#;
        assert!(matches!(
            RepositorySyncPolicyV1::from_snapshot(bytes),
            Err(PolicyError::InvalidJson(_))
        ));
    }

    #[test]
    fn parser_rejects_traversal_and_oversized_snapshots() {
        let mut policy = repository_policy();
        policy.monitored_roots = vec!["../outside".into()];
        let bytes = serde_json::to_vec(&policy).unwrap();
        assert_eq!(
            RepositorySyncPolicyV1::from_snapshot(&bytes),
            Err(PolicyError::InvalidRepositoryPattern("../outside".into()))
        );

        let oversized = vec![b' '; MAX_POLICY_BYTES + 1];
        assert!(matches!(
            RepositorySyncPolicyV1::from_snapshot(&oversized),
            Err(PolicyError::TooLarge { .. })
        ));
    }
}
