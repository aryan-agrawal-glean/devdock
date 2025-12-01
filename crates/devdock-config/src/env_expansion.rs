//! Environment variable expansion for configuration values.

use regex::Regex;
use std::collections::HashMap;

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
    map.iter()
        .map(|(k, v)| (k.clone(), expand_env_vars(v)))
        .collect()
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
        assert_eq!(
            expand_env_vars("${VAR1}/middle/${VAR2}"),
            "val1/middle/val2"
        );
    }
}


