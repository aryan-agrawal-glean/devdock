//! Opt-in compatibility check for a real loopback-forwarded Tilt instance.
//!
//! This is ignored in CI because it requires a live Tilt server. It never
//! discovers or reads Tilt credential files.

use devdock_monitor::TiltClient;

#[test]
#[ignore = "requires DEVDOCK_TILT_TEST_ENDPOINT pointing at a live loopback Tilt server"]
fn real_tilt_web_contract() {
    let endpoint = std::env::var("DEVDOCK_TILT_TEST_ENDPOINT")
        .expect("set DEVDOCK_TILT_TEST_ENDPOINT=http://127.0.0.1:<port>");
    let client = TiltClient::new(&endpoint).expect("create loopback Tilt client");
    let snapshot = client.get_snapshot().expect("fetch and validate /api/view");

    println!("endpoint={endpoint}");
    println!("tilt_start_time={}", snapshot.tilt_start_time);
    println!("resource_count={}", snapshot.resources.len());
    println!("response_bytes={}", snapshot.response_bytes);
    println!("request_duration_us={}", snapshot.request_duration.as_micros());
    println!("parse_duration_us={}", snapshot.parse_duration.as_micros());
    println!("view_request_count={}", snapshot.view_request_count);
    for resource in &snapshot.resources {
        let latest_build =
            resource.build_history.first().map(|build| build.span_id.as_str()).unwrap_or("none");
        println!(
            "resource={} status={:?} runtime_status={} update_status={} latest_build={}",
            resource.name,
            resource.status,
            resource.runtime_status.as_deref().unwrap_or("unknown"),
            resource.update_status.as_deref().unwrap_or("unknown"),
            latest_build
        );
    }

    if let Ok(resource) = std::env::var("DEVDOCK_TILT_TRIGGER_RESOURCE") {
        assert!(
            snapshot.resources.iter().any(|candidate| candidate.name == resource),
            "refusing to trigger a resource absent from the validated snapshot"
        );
        client.trigger_resource(&resource).expect("trigger validated Tilt resource");
        println!("triggered_resource={resource}");
    }
}
