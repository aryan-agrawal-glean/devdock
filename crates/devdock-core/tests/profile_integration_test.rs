use devdock_core::profile::Profile;

#[test]
fn test_profile_with_multi_project_loads_all_services() {
    let yaml = r#"
project: test-project

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

multi_project:
  project: secondary-project
  infra:
    sql:
      type: local
  services:
    internal_admin:
      type: remote
"#;

    let profile: Profile = serde_yaml::from_str(yaml).unwrap();

    // Verify profile loads correctly
    assert_eq!(profile.project, "test-project");

    // Get all components
    let all_comps = profile.all_components();

    // Verify top-level services are included
    assert!(profile.has_component("qe"), "Top-level service 'qe' should be detected");

    // Verify multi-project services are included
    assert!(
        profile.has_component("internal_admin"),
        "Multi-project service 'internal_admin' should be detected"
    );

    // Verify infrastructure components
    assert!(profile.has_component("sql"), "Infrastructure 'sql' should be detected");
    assert!(profile.has_component("redis"), "Infrastructure 'redis' should be detected");

    // Verify multi-project config
    let multi = profile.multi_project.as_ref().unwrap();
    assert_eq!(multi.project, "secondary-project");

    // Verify component count: sql, redis, qe, internal_admin = 4 unique
    assert_eq!(all_comps.len(), 4);
}
