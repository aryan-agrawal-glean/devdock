//! INI configuration parser for DevDock.
//!
//! Parses INI files like `default.ini` and provides structured access to sections.
//! Compatible with Python's `configparser.RawConfigParser`.

use anyhow::{Context, Result};
use ini::Ini;
use std::collections::HashMap;
use std::path::Path;

/// Parsed INI configuration with section-based access
#[derive(Debug, Clone, Default)]
pub struct ParsedIniConfig {
    sections: HashMap<String, HashMap<String, String>>,
}

impl ParsedIniConfig {
    /// Create a new empty config
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse INI from a file path
    pub fn from_file(path: &Path) -> Result<Self> {
        let ini = Ini::load_from_file(path)
            .with_context(|| format!("Failed to parse INI file: {}", path.display()))?;

        Ok(Self::from_ini(ini))
    }

    /// Parse INI from a string
    pub fn parse_str(content: &str) -> Result<Self> {
        let ini = Ini::load_from_str(content).context("Failed to parse INI content")?;
        Ok(Self::from_ini(ini))
    }

    /// Parse INI from bytes (handles potential BOM and encoding)
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let content = String::from_utf8_lossy(bytes);
        // Strip UTF-8 BOM if present
        let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
        Self::parse_str(content)
    }

    fn from_ini(ini: Ini) -> Self {
        let mut sections = HashMap::new();

        for (section_name, properties) in &ini {
            let section_key = section_name.map(|s| s.to_string()).unwrap_or_default();
            let mut section_map = HashMap::new();

            for (key, value) in properties {
                section_map.insert(key.to_string(), value.to_string());
            }

            sections.insert(section_key, section_map);
        }

        Self { sections }
    }

    /// Get a value from a section
    pub fn get(&self, section: &str, key: &str) -> Option<&String> {
        self.sections.get(section)?.get(key)
    }

    /// Get a value with a default
    pub fn get_or_default<'a>(&'a self, section: &str, key: &str, default: &'a str) -> &'a str {
        self.get(section, key)
            .map(|s| s.as_str())
            .unwrap_or(default)
    }

    /// Check if a section exists
    pub fn has_section(&self, section: &str) -> bool {
        self.sections.contains_key(section)
    }

    /// Get all keys in a section
    pub fn section_keys(&self, section: &str) -> Option<impl Iterator<Item = &String>> {
        self.sections.get(section).map(|s| s.keys())
    }

    /// Get all key-value pairs in a section
    pub fn section(&self, section: &str) -> Option<&HashMap<String, String>> {
        self.sections.get(section)
    }

    /// Get all section names
    pub fn sections(&self) -> impl Iterator<Item = &String> {
        self.sections.keys()
    }

    /// Set a value (for merging configs)
    pub fn set(&mut self, section: &str, key: &str, value: impl Into<String>) {
        self.sections
            .entry(section.to_string())
            .or_default()
            .insert(key.to_string(), value.into());
    }

    /// Merge another config into this one (other takes precedence)
    pub fn merge(&mut self, other: &ParsedIniConfig) {
        for (section, keys) in &other.sections {
            for (key, value) in keys {
                self.set(section, key, value.clone());
            }
        }
    }

    /// Get all options that match a suffix pattern in a section
    /// e.g., options ending with ".db.instance" in the "sql" section
    pub fn get_options_by_suffix(&self, section: &str, suffix: &str) -> Vec<(String, String)> {
        self.sections
            .get(section)
            .map(|s| {
                s.iter()
                    .filter(|(k, _)| k.ends_with(suffix))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all options that match a prefix pattern in a section
    pub fn get_options_by_prefix(&self, section: &str, prefix: &str) -> Vec<(String, String)> {
        self.sections
            .get(section)
            .map(|s| {
                s.iter()
                    .filter(|(k, _)| k.starts_with(prefix))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Extract database names from SQL section (like Python's process_instance)
    pub fn get_sql_databases_by_instance(&self, instance_type: &str) -> Vec<String> {
        let mut dbs = Vec::new();

        if let Some(sql_section) = self.sections.get("sql") {
            for (option, value) in sql_section {
                if option.ends_with(".db.instance") && value == instance_type {
                    // Extract database name from option like "frontend_keyval.db.instance"
                    if let Some(db_name) = option.strip_suffix(".db.instance") {
                        dbs.push(db_name.to_string());
                    }
                }
            }
        }

        dbs
    }

    /// Convert back to INI string
    pub fn as_string(&self) -> String {
        let mut result = String::new();

        // Sort sections for deterministic output
        let mut section_names: Vec<_> = self.sections.keys().collect();
        section_names.sort();

        for section in section_names {
            if let Some(keys) = self.sections.get(section) {
                if !section.is_empty() {
                    result.push_str(&format!("[{}]\n", section));
                }

                // Sort keys for deterministic output
                let mut key_names: Vec<_> = keys.keys().collect();
                key_names.sort();

                for key in key_names {
                    if let Some(value) = keys.get(key) {
                        result.push_str(&format!("{} = {}\n", key, value));
                    }
                }

                result.push('\n');
            }
        }

        result
    }
}

/// Parse SQL database/table query string (like Python's parse_db_tables_query)
/// Format: "db:tables:query" or "db:tables" or "db"
pub fn parse_db_tables_query(db_tables_query: &str) -> (String, String, Option<String>) {
    let parts: Vec<&str> = db_tables_query.split(':').collect();

    let db = parts.first().map(|s| s.to_string()).unwrap_or_default();
    let tables = parts.get(1).map(|s| s.to_string()).unwrap_or_default();
    let query = if parts.len() >= 3 {
        Some(parts[2..].join(":"))
    } else {
        None
    };

    (db, tables, query)
}

/// Parse semicolon-delimited database specifications
/// Format: "db1;db2:tables;db3:tables:query"
pub fn parse_db_specs(specs: &str) -> Vec<(String, String, Option<String>)> {
    specs
        .split(';')
        .filter(|s| !s.trim().is_empty())
        .map(|s| parse_db_tables_query(s.trim()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_INI: &str = r#"
[sql]
useSecretsManagerForPassword = false
frontend_keyval.db.instance = frontendInstance
backend_data.db.instance = backendInstance
test.db.instance = frontendInstance

[google]
storage.configbucket = config-glean-prod
apps.useCentralApp = false

[memcached]
discoveryEndpoints = memcached.devdock.local:11211
"#;

    #[test]
    fn test_parse_ini_string() {
        let config = ParsedIniConfig::parse_str(SAMPLE_INI).unwrap();

        assert!(config.has_section("sql"));
        assert!(config.has_section("google"));
        assert!(config.has_section("memcached"));

        assert_eq!(
            config.get("sql", "useSecretsManagerForPassword"),
            Some(&"false".to_string())
        );
        assert_eq!(
            config.get("google", "storage.configbucket"),
            Some(&"config-glean-prod".to_string())
        );
    }

    #[test]
    fn test_get_sql_databases_by_instance() {
        let config = ParsedIniConfig::parse_str(SAMPLE_INI).unwrap();

        let frontend_dbs = config.get_sql_databases_by_instance("frontendInstance");
        assert!(frontend_dbs.contains(&"frontend_keyval".to_string()));
        assert!(frontend_dbs.contains(&"test".to_string()));
        assert_eq!(frontend_dbs.len(), 2);

        let backend_dbs = config.get_sql_databases_by_instance("backendInstance");
        assert!(backend_dbs.contains(&"backend_data".to_string()));
        assert_eq!(backend_dbs.len(), 1);
    }

    #[test]
    fn test_get_options_by_suffix() {
        let config = ParsedIniConfig::parse_str(SAMPLE_INI).unwrap();

        let db_instances = config.get_options_by_suffix("sql", ".db.instance");
        assert_eq!(db_instances.len(), 3);
    }

    #[test]
    fn test_parse_db_tables_query() {
        let (db, tables, query) = parse_db_tables_query("mydb");
        assert_eq!(db, "mydb");
        assert_eq!(tables, "");
        assert!(query.is_none());

        let (db, tables, query) = parse_db_tables_query("mydb:table1,table2");
        assert_eq!(db, "mydb");
        assert_eq!(tables, "table1,table2");
        assert!(query.is_none());

        let (db, tables, query) = parse_db_tables_query("mydb:table1:id > 100");
        assert_eq!(db, "mydb");
        assert_eq!(tables, "table1");
        assert_eq!(query, Some("id > 100".to_string()));
    }

    #[test]
    fn test_merge_configs() {
        let mut base = ParsedIniConfig::parse_str(
            r#"
[section1]
key1 = value1
key2 = value2
"#,
        )
        .unwrap();

        let override_config = ParsedIniConfig::parse_str(
            r#"
[section1]
key2 = overridden
key3 = new_value
"#,
        )
        .unwrap();

        base.merge(&override_config);

        assert_eq!(base.get("section1", "key1"), Some(&"value1".to_string()));
        assert_eq!(
            base.get("section1", "key2"),
            Some(&"overridden".to_string())
        );
        assert_eq!(base.get("section1", "key3"), Some(&"new_value".to_string()));
    }

    #[test]
    fn test_to_string() {
        let mut config = ParsedIniConfig::new();
        config.set("section1", "key1", "value1");
        config.set("section1", "key2", "value2");
        config.set("section2", "keyA", "valueA");

        let output = config.as_string();
        assert!(output.contains("[section1]"));
        assert!(output.contains("key1 = value1"));
        assert!(output.contains("[section2]"));
        assert!(output.contains("keyA = valueA"));
    }
}
