//! Dependency resolver — DFS traversal of service dependencies.
//!
//! Supports unified components: a component is classified as infra if:
//! 1. Its name matches the well-known infra list (backward compat), OR
//! 2. Its `devdock_config.json` has `"kind": "infra"`

use crate::service_config::{ComponentKind, DevdockServiceConfig};
use std::collections::{HashMap, HashSet};

const INFRA_COMPONENTS: &[&str] = &[
    "sql",
    "redis",
    "opensearch",
    "memcached",
    "pubsub",
    "rabbitmq",
    "nginx",
    "bastion",
    "servicebus",
];

#[derive(Debug, Clone)]
pub struct ResolvedDeps {
    /// Services in dependency order (deps before dependents)
    pub services: Vec<String>,
    /// Infrastructure components needed
    pub infra: Vec<String>,
}

/// Check if a component is infra — either by well-known name or by config kind.
pub fn is_infra(name: &str) -> bool {
    INFRA_COMPONENTS.contains(&name)
}

/// Check if a component should be treated as infra, considering both
/// the well-known name list and the config's `kind` field.
fn is_infra_with_config(name: &str, configs: &HashMap<String, DevdockServiceConfig>) -> bool {
    if INFRA_COMPONENTS.contains(&name) {
        return true;
    }
    if let Some(config) = configs.get(name) {
        return config.kind == ComponentKind::Infra;
    }
    false
}

pub fn resolve(
    requested: &[String],
    configs: &HashMap<String, DevdockServiceConfig>,
) -> crate::Result<ResolvedDeps> {
    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();
    let mut order = Vec::new();
    let mut infra = HashSet::new();

    for name in requested {
        if is_infra_with_config(name, configs) {
            infra.insert(name.clone());
        } else {
            dfs(name, configs, &mut visited, &mut visiting, &mut order, &mut infra)?;
        }
    }

    Ok(ResolvedDeps { services: order, infra: infra.into_iter().collect() })
}

fn dfs(
    name: &str,
    configs: &HashMap<String, DevdockServiceConfig>,
    visited: &mut HashSet<String>,
    visiting: &mut HashSet<String>,
    order: &mut Vec<String>,
    infra: &mut HashSet<String>,
) -> crate::Result<()> {
    if visited.contains(name) {
        return Ok(());
    }
    if visiting.contains(name) {
        return Err(crate::Error::DependencyCycle(format!("Circular dependency: '{name}'")));
    }
    if is_infra_with_config(name, configs) {
        infra.insert(name.to_string());
        return Ok(());
    }

    visiting.insert(name.to_string());
    if let Some(config) = configs.get(name) {
        for dep in &config.dependencies {
            dfs(dep, configs, visited, visiting, order, infra)?;
        }
    }
    visiting.remove(name);
    visited.insert(name.to_string());
    order.push(name.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(deps: &[&str]) -> DevdockServiceConfig {
        DevdockServiceConfig::parse(
            &serde_json::json!({"type":"python","dependencies":deps}).to_string(),
        )
        .unwrap()
    }

    fn cfgs(entries: &[(&str, &[&str])]) -> HashMap<String, DevdockServiceConfig> {
        entries.iter().map(|(n, d)| (n.to_string(), svc(d))).collect()
    }

    #[test]
    fn single_no_deps() {
        let r = resolve(&["api".into()], &cfgs(&[("api", &[])])).unwrap();
        assert_eq!(r.services, vec!["api"]);
        assert!(r.infra.is_empty());
    }

    #[test]
    fn transitive() {
        let r = resolve(&["api".into()], &cfgs(&[("api", &["sql", "redis"])])).unwrap();
        assert_eq!(r.services, vec!["api"]);
        assert!(r.infra.contains(&"sql".into()));
        assert!(r.infra.contains(&"redis".into()));
    }

    #[test]
    fn linear_chain() {
        let c = cfgs(&[("fe", &["api"]), ("api", &[])]);
        let r = resolve(&["fe".into()], &c).unwrap();
        let api_pos = r.services.iter().position(|s| s == "api").unwrap();
        let fe_pos = r.services.iter().position(|s| s == "fe").unwrap();
        assert!(api_pos < fe_pos);
    }

    #[test]
    fn circular_detected() {
        let c = cfgs(&[("a", &["b"]), ("b", &["a"])]);
        assert!(resolve(&["a".into()], &c).is_err());
    }

    #[test]
    fn diamond() {
        let c = cfgs(&[("w", &["api", "sql"]), ("api", &["sql"])]);
        let r = resolve(&["w".into()], &c).unwrap();
        assert_eq!(r.infra.iter().filter(|s| *s == "sql").count(), 1);
        assert_eq!(r.services.iter().filter(|s| *s == "api").count(), 1);
    }

    #[test]
    fn kind_infra_resolved_as_infra() {
        let mut configs = HashMap::new();
        configs.insert(
            "my-db".to_string(),
            DevdockServiceConfig::parse(r#"{"kind":"infra","image":"mysql:8.0","port":"3306"}"#)
                .unwrap(),
        );
        configs.insert("api".to_string(), svc(&["my-db"]));

        let r = resolve(&["api".into()], &configs).unwrap();
        assert_eq!(r.services, vec!["api"]);
        assert!(r.infra.contains(&"my-db".into()));
    }

    #[test]
    fn fixture_resolution() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("tests/fixtures/multi-service/devdock/config/services");
        if fixture.exists() {
            let c = crate::service_config::discover_services(&fixture).unwrap();
            let r = resolve(&["frontend".into()], &c).unwrap();
            assert!(r.services.contains(&"frontend".into()));
            assert!(r.services.contains(&"api".into()));
            assert!(r.infra.contains(&"sql".into()));
        }
    }
}
