//! Backward-compatible SQL table and KeyVal policy normalization.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

/// The table identity used by the local SQL hydrator for KeyVal.
pub const KEYVAL_TABLE_IDENTITY: &str = "keyval:KeyVal";

/// Presence-aware KeyVal policy.
///
/// `Auto` retains the existing omitted/legacy behavior during the expansion
/// phase. It is intentionally not contracted to `Never` here; that belongs to
/// a later Scio migration after compatibility evidence exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyValPolicy {
    #[default]
    Auto,
    Always,
    Never,
}

impl KeyValPolicy {
    /// Map an explicit legacy boolean without losing presence information.
    pub const fn from_legacy_bool(copy_keyval: bool) -> Self {
        if copy_keyval {
            Self::Always
        } else {
            Self::Never
        }
    }

    pub const fn allows_keyval(self) -> bool {
        !matches!(self, Self::Never)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        }
    }

    /// Parse the new string form or an old boolean JSON value.
    pub fn from_value(value: Option<&Value>) -> Result<Self> {
        match value {
            None | Some(Value::Null) => Ok(Self::Auto),
            Some(Value::Bool(value)) => Ok(Self::from_legacy_bool(*value)),
            Some(Value::String(value)) => match value.trim().to_ascii_lowercase().as_str() {
                "auto" => Ok(Self::Auto),
                "always" => Ok(Self::Always),
                "never" => Ok(Self::Never),
                _ => Err(anyhow!("invalid KeyVal policy; expected auto, always, or never")),
            },
            Some(_) => Err(anyhow!("invalid KeyVal policy; expected a string or boolean")),
        }
    }
}

/// The normalized table copy plan consumed by the SQL hydrator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCopyPlan {
    pub policy: KeyValPolicy,
    pub entries: Vec<String>,
}

impl TableCopyPlan {
    pub fn as_spec(&self) -> String {
        self.entries.join(";")
    }

    pub fn includes_keyval(&self) -> bool {
        self.entries.iter().any(|entry| is_keyval_identity(entry))
    }
}

/// Normalize table identities while preserving their order.
///
/// Each semicolon-delimited entry is an identity, not a substring. Database
/// and table names are trimmed, table lists are deduplicated and sorted, and
/// duplicate entries are removed exactly once. Query text remains part of the
/// identity and is only trimmed at its boundaries.
pub fn normalize_table_identities(spec: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();

    for raw_entry in spec.split(';') {
        let entry = canonical_table_identity(raw_entry);
        if entry.is_empty() || !seen.insert(entry.clone()) {
            continue;
        }
        entries.push(entry);
    }

    entries
}

/// Resolve a profile/CLI table specification into one exact plan.
pub fn resolve_table_copy_plan(
    spec: &str,
    policy: KeyValPolicy,
    force_copy: bool,
) -> TableCopyPlan {
    let mut entries = normalize_table_identities(spec);

    // `never` is a hard boundary: it cannot add or refresh KeyVal, including
    // when an older profile explicitly left the table in copy_tables.
    if matches!(policy, KeyValPolicy::Never) {
        entries.retain(|entry| !is_keyval_identity(entry));
    } else if !force_copy && !entries.iter().any(|entry| is_keyval_identity(entry)) {
        entries.push(KEYVAL_TABLE_IDENTITY.to_string());
    }

    TableCopyPlan { policy, entries }
}

/// Whether a normalized entry names the exact KeyVal table.
pub fn is_keyval_identity(entry: &str) -> bool {
    let mut parts = entry.splitn(3, ':');
    matches!((parts.next(), parts.next(), parts.next()), (Some("keyval"), Some("KeyVal"), None))
}

fn canonical_table_identity(raw_entry: &str) -> String {
    let raw_entry = raw_entry.trim();
    if raw_entry.is_empty() {
        return String::new();
    }

    let mut parts = raw_entry.splitn(3, ':');
    let database = parts.next().unwrap_or_default().trim();
    let tables = parts.next().map(str::trim);
    let query = parts.next().map(str::trim);

    if database.is_empty() {
        return String::new();
    }

    let Some(tables) = tables else {
        return database.to_string();
    };

    let mut table_names =
        tables.split(',').map(str::trim).filter(|table| !table.is_empty()).collect::<Vec<_>>();
    table_names.sort_unstable();
    table_names.dedup();
    let tables = table_names.join(",");

    match query.filter(|query| !query.is_empty()) {
        Some(query) => format!("{database}:{tables}:{query}"),
        None if tables.is_empty() => database.to_string(),
        None => format!("{database}:{tables}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_legacy_booleans_and_new_values() {
        assert_eq!(KeyValPolicy::from_value(None).unwrap(), KeyValPolicy::Auto);
        assert_eq!(
            KeyValPolicy::from_value(Some(&Value::Bool(true))).unwrap(),
            KeyValPolicy::Always
        );
        assert_eq!(
            KeyValPolicy::from_value(Some(&Value::Bool(false))).unwrap(),
            KeyValPolicy::Never
        );
        assert_eq!(
            KeyValPolicy::from_value(Some(&Value::String("never".into()))).unwrap(),
            KeyValPolicy::Never
        );
    }

    #[test]
    fn normalizes_and_deduplicates_exact_table_identities() {
        assert_eq!(
            normalize_table_identities(
                " entity: b, a ;entity:a,b;keyval:KeyVal;not-keyval:KeyVal;keyval:KeyVal"
            ),
            vec!["entity:a,b", "keyval:KeyVal", "not-keyval:KeyVal",]
        );
    }

    #[test]
    fn auto_and_always_add_keyval_once_but_never_does_not_add_or_refresh() {
        let auto = resolve_table_copy_plan("entity;keyval:KeyVal", KeyValPolicy::Auto, false);
        assert_eq!(auto.entries, vec!["entity", KEYVAL_TABLE_IDENTITY]);

        let always = resolve_table_copy_plan("entity", KeyValPolicy::Always, false);
        assert_eq!(always.entries, vec!["entity", KEYVAL_TABLE_IDENTITY]);

        let never = resolve_table_copy_plan(
            "entity;keyval:KeyVal;keyval:KeyVal",
            KeyValPolicy::Never,
            false,
        );
        assert_eq!(never.entries, vec!["entity"]);
        assert!(!never.includes_keyval());
    }
}
