use devdock_core::profile::Profile;

#[test]
fn test_profile_with_glean_apps_canary_loads_all_services() {
    let yaml = r#"
project: dev-sandbox-334901

infra:
  sql:
    type: local
    copy_tables: keyval:KeyVal;entity
    mode: fast
  redis:
    type: local

services:
  qe:
    type: local
    env:
      DEBUG_MODE: 'True'
    hot_reload: enable

glean-apps-canary:
  infra:
    sql:
      type: local
  services:
    internal_admin:
      type: remote
"#;

    let profile: Profile = serde_yaml::from_str(yaml).unwrap();

    // Verify profile loads correctly
    assert_eq!(profile.project, "dev-sandbox-334901");

    // Get all components
    let all_comps = profile.all_components();

    // Verify top-level services are included
    assert!(profile.has_component("qe"), "Top-level service 'qe' should be detected");
    assert!(all_comps.contains(&"qe".to_string()), "all_components should include 'qe'");

    // Verify multi-project services are included
    assert!(
        profile.has_component("internal_admin"),
        "Multi-project service 'internal_admin' should be detected"
    );
    assert!(
        all_comps.contains(&"internal_admin".to_string()),
        "all_components should include 'internal_admin'"
    );

    // Verify infrastructure components
    assert!(profile.has_component("sql"), "Infrastructure 'sql' should be detected");
    assert!(profile.has_component("redis"), "Infrastructure 'redis' should be detected");

    // Verify component count
    // Top-level: sql, redis, qe (3 components)
    // Multi-project: internal_admin (1 new component, sql is duplicate)
    // Total unique: 4 components
    assert_eq!(
        all_comps.len(),
        4,
        "Should have exactly 4 unique components, got {}: {:?}",
        all_comps.len(),
        all_comps
    );

    // Verify sql appears only once despite being in both sections
    let sql_count = all_comps.iter().filter(|c| *c == "sql").count();
    assert_eq!(
        sql_count, 1,
        "sql should appear only once in all_components, found {} times",
        sql_count
    );

    println!("✅ All components loaded correctly (deduplicated): {:?}", all_comps);
}
