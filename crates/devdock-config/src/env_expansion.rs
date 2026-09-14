//! Environment variable expansion for configuration values.

use regex::Regex;
use std::collections::HashMap;
use thiserror::Error;

/// Errors returned when an identity-bearing value cannot be resolved safely.
///
/// The error deliberately contains only the variable name or a generic syntax
/// message. It never includes the input value, so a profile cannot leak a
/// secret-bearing value through a resolution diagnostic.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdentityResolutionError {
    #[error("unresolved identity variable: {name}")]
    UnresolvedVariable { name: String },

    #[error("invalid identity variable placeholder")]
    InvalidPlaceholder,
}

fn valid_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Resolve the documented `${VAR}` and `${VAR:-default}` forms for an
/// identity-bearing value.
///
/// Unlike [`expand_env_vars`], this function is strict: an unset variable
/// without a default, malformed placeholder, or empty resolved value is not
/// silently passed to a downstream consumer. Empty values are returned so the
/// profile validator can apply its normal required-field check.
pub fn resolve_identity(value: &str) -> Result<String, IdentityResolutionError> {
    if !value.contains("${") {
        return Ok(value.to_string());
    }

    let mut resolved = String::with_capacity(value.len());
    let mut remaining = value;

    while let Some(start) = remaining.find("${") {
        resolved.push_str(&remaining[..start]);
        let placeholder = &remaining[start + 2..];
        let Some(end) = placeholder.find('}') else {
            return Err(IdentityResolutionError::InvalidPlaceholder);
        };

        let expression = &placeholder[..end];
        let (name, default) = expression
            .split_once(":-")
            .map_or((expression, None), |(name, default)| (name, Some(default)));
        if !valid_variable_name(name) {
            return Err(IdentityResolutionError::InvalidPlaceholder);
        }

        let env_value = std::env::var(name).ok();
        match (env_value, default) {
            (Some(value), Some(default)) if value.is_empty() => resolved.push_str(default),
            (Some(value), _) => resolved.push_str(&value),
            (None, Some(default)) => resolved.push_str(default),
            (None, None) => {
                return Err(IdentityResolutionError::UnresolvedVariable { name: name.to_string() })
            }
        }

        remaining = &placeholder[end + 1..];
    }

    resolved.push_str(remaining);
    Ok(resolved)
}

/// Expand environment variables in a string
/// Supports both ${VAR} and ${VAR:-default} syntax
pub fn expand_env_vars(value: &str) -> String {
    if !value.contains("${") {
        return value.to_string();
    }

    let mut result = value.to_string();

    // Handle ${VAR:-default} syntax first
    let re_with_default = Regex::new(r"\$\{([^:]+):-([^}]+)\}").unwrap();
    for cap in re_with_default.captures_iter(value) {
        let var_name = &cap[1];
        let default_value = &cap[2];
        let env_value = std::env::var(var_name).unwrap_or_else(|_| default_value.to_string());
        result = result.replace(&cap[0], &env_value);
    }

    // Handle simple ${VAR} syntax
    let re_simple = Regex::new(r"\$\{([^}]+)\}").unwrap();
    for cap in re_simple.captures_iter(&result.clone()) {
        let var_name = &cap[1];
        if let Ok(env_value) = std::env::var(var_name) {
            result = result.replace(&cap[0], &env_value);
        }
        // If var not found, leave as-is
    }

    result
}

/// Expand environment variables in a HashMap recursively
pub fn expand_env_vars_in_map(map: &HashMap<String, String>) -> HashMap<String, String> {
    map.iter().map(|(k, v)| (k.clone(), expand_env_vars(v))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_simple() {
        std::env::set_var("TEST_VAR", "test_value");
        assert_eq!(expand_env_vars("prefix_${TEST_VAR}_suffix"), "prefix_test_value_suffix");
    }

    #[test]
    fn test_expand_with_default() {
        std::env::remove_var("MISSING_VAR");
        assert_eq!(expand_env_vars("${MISSING_VAR:-default}"), "default");
    }

    #[test]
    fn test_expand_existing_with_default() {
        std::env::set_var("EXISTING_VAR", "actual");
        assert_eq!(expand_env_vars("${EXISTING_VAR:-default}"), "actual");
    }

    #[test]
    fn test_no_expansion() {
        assert_eq!(expand_env_vars("plain_text"), "plain_text");
    }

    #[test]
    fn test_multiple_vars() {
        std::env::set_var("VAR1", "val1");
        std::env::set_var("VAR2", "val2");
        assert_eq!(expand_env_vars("${VAR1}/middle/${VAR2}"), "val1/middle/val2");
    }

    #[test]
    fn resolve_identity_supports_literal_and_set_values() {
        std::env::set_var("DEVDock_TEST_PROJECT", "resolved-project");
        assert_eq!(resolve_identity("literal-project").unwrap(), "literal-project");
        assert_eq!(resolve_identity("${DEVDock_TEST_PROJECT}").unwrap(), "resolved-project");
    }

    #[test]
    fn resolve_identity_uses_default_for_missing_or_empty_values() {
        std::env::remove_var("DEVDock_MISSING_PROJECT");
        std::env::set_var("DEVDock_EMPTY_PROJECT", "");
        assert_eq!(
            resolve_identity("${DEVDock_MISSING_PROJECT:-default-project}").unwrap(),
            "default-project"
        );
        assert_eq!(
            resolve_identity("${DEVDock_EMPTY_PROJECT:-default-project}").unwrap(),
            "default-project"
        );
    }

    #[test]
    fn resolve_identity_rejects_unresolved_values_without_echoing_input() {
        std::env::remove_var("DEVDock_SECRET_PROJECT");
        let error = resolve_identity("prefix-${DEVDock_SECRET_PROJECT}-suffix").unwrap_err();
        assert_eq!(
            error,
            IdentityResolutionError::UnresolvedVariable {
                name: "DEVDock_SECRET_PROJECT".to_string()
            }
        );
        assert!(!error.to_string().contains("prefix-"));
    }

    #[test]
    fn resolve_identity_rejects_malformed_placeholders() {
        assert_eq!(
            resolve_identity("${BAD-NAME}").unwrap_err(),
            IdentityResolutionError::InvalidPlaceholder
        );
        assert_eq!(
            resolve_identity("${MISSING").unwrap_err(),
            IdentityResolutionError::InvalidPlaceholder
        );
    }
}
