//! Benchmark integration test for measuring startup time regressions.
//!
//! This test is marked with #[ignore] and only runs when explicitly requested:
//!   cargo test --release benchmark_startup -- --ignored --nocapture
//!
//! It measures the time to execute all startup logic up to (but not including)
//! spawning the Tilt process, using the --dry-run flag.

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

#[test]
#[ignore] // Only run when explicitly requested
fn benchmark_startup_bootstrap_time() {
    // Get the path to the built binary (assume it's already built by cargo test)
    let workspace_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf();

    // Use debug binary by default (CI runs in debug mode for speed)
    let binary_path = if cfg!(debug_assertions) {
        workspace_root.join("target/debug/devdock")
    } else {
        workspace_root.join("target/release/devdock")
    };

    assert!(binary_path.exists(), "Binary not found at {}", binary_path.display());

    // Set up test fixture directory
    let test_fixture = workspace_root.join("tests/fixtures/benchmark");
    assert!(test_fixture.exists(), "Test fixture not found at {}", test_fixture.display());

    // Dry-run startup still performs legacy startup-state cleanup. Isolate the benchmark from the
    // developer/runner's real ~/.devdock tree so the measurement is deterministic and read-only
    // with respect to existing sessions.
    let benchmark_home = tempfile::tempdir().expect("Failed to create isolated benchmark home");

    // Run the benchmark
    let start = Instant::now();
    let output = Command::new(&binary_path)
        .args(["start", "bench", "--dry-run", "--ci-mode"])
        .current_dir(&test_fixture)
        .env("HOME", benchmark_home.path())
        .output()
        .expect("Failed to execute devdock start");

    let elapsed = start.elapsed();

    // Parse JSON output
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Print outputs for debugging
    println!("=== STDOUT ===");
    println!("{}", stdout);
    println!("=== STDERR ===");
    println!("{}", stderr);

    // Check if the command succeeded
    if !output.status.success() {
        panic!(
            "Command failed with status: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
            output.status, stdout, stderr
        );
    }

    // Extract JSON timing from output
    let json_line = stdout
        .lines()
        .find(|line| line.contains("bootstrap_time_ms"))
        .expect("Could not find JSON timing output");

    let timing: serde_json::Value =
        serde_json::from_str(json_line).expect("Failed to parse JSON timing output");

    let bootstrap_time_ms =
        timing["bootstrap_time_ms"].as_i64().expect("bootstrap_time_ms not found in JSON");

    // Print benchmark results
    println!("\n=== BENCHMARK RESULTS ===");
    println!("Profile: {}", timing["profile"].as_str().unwrap());
    println!("Bootstrap time: {} ms", bootstrap_time_ms);
    println!("Elapsed (wall clock): {:?}", elapsed);
    println!("=========================\n");

    // Sanity check: bootstrap time should be reasonable (< 60 seconds for minimal profile)
    assert!(
        bootstrap_time_ms < 60_000,
        "Bootstrap time too high: {} ms (expected < 60000 ms)",
        bootstrap_time_ms
    );

    // Sanity check: bootstrap time should be positive
    assert!(
        bootstrap_time_ms > 0,
        "Bootstrap time should be positive, got: {} ms",
        bootstrap_time_ms
    );
}
