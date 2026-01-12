//! Benchmark integration test for measuring startup time regressions.
//!
//! This test is marked with #[ignore] and only runs when explicitly requested:
//!   cargo test --release benchmark_startup -- --ignored --nocapture
//!
//! It measures the time to execute all startup logic up to (but not including)
//! spawning the Tilt process, using the --dry-run flag.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[test]
#[ignore] // Only run when explicitly requested
fn benchmark_startup_bootstrap_time() {
    // Setup: Build the devdock binary in release mode
    let cargo_build = Command::new("cargo")
        .args(["build", "--release", "--bin", "devdock"])
        .output()
        .expect("Failed to build devdock binary");

    assert!(
        cargo_build.status.success(),
        "Failed to build devdock: {}",
        String::from_utf8_lossy(&cargo_build.stderr)
    );

    // Get the path to the built binary
    let workspace_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf();

    let binary_path = workspace_root.join("target/release/devdock");
    assert!(binary_path.exists(), "Binary not found at {}", binary_path.display());

    // Set up test fixture directory
    let test_fixture = workspace_root.join("tests/fixtures/benchmark");
    assert!(test_fixture.exists(), "Test fixture not found at {}", test_fixture.display());

    // Run the benchmark with a timeout to prevent hanging
    let start = Instant::now();

    let (tx, rx) = mpsc::channel();
    let binary_path_clone = binary_path.clone();
    let test_fixture_clone = test_fixture.clone();
    let home = std::env::var("HOME").unwrap();

    // Spawn the command in a separate thread
    thread::spawn(move || {
        let output = Command::new(&binary_path_clone)
            .args(["start", "bench", "--dry-run", "--ci-mode"])
            .current_dir(&test_fixture_clone)
            .env("HOME", home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        tx.send(output).ok();
    });

    // Wait with a 20-second timeout
    let output = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("Command timed out after 20 seconds")
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
