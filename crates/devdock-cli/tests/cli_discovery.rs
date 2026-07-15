use std::process::Command;
use tempfile::tempdir;

fn devdock() -> Command {
    Command::new(env!("CARGO_BIN_EXE_devdock"))
}

#[test]
fn start_help_exposes_remote_alias_and_legacy_mode() {
    let output = devdock().args(["start", "--help"]).output().expect("run devdock start --help");

    assert!(output.status.success(), "start --help failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("--remote"), "start help should expose --remote alias:\n{stdout}");
    assert!(
        stdout.contains("--mode <MODE>"),
        "start help should keep legacy --mode option:\n{stdout}"
    );
    assert!(
        stdout.contains("[default: local]"),
        "start help should keep local as default:\n{stdout}"
    );
}

#[test]
fn invalid_start_mode_is_rejected_by_clap() {
    let output = devdock()
        .args(["start", "example-profile", "--mode", "remtoe", "--dry-run"])
        .output()
        .expect("run devdock with invalid mode");

    assert!(!output.status.success(), "invalid mode should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains("invalid value") || stderr.contains("possible values"),
        "unexpected stderr:\n{stderr}"
    );
}

#[test]
fn cleanup_succeeds_without_docker() {
    let home = tempdir().expect("create temporary HOME");
    let workspace = tempdir().expect("create temporary workspace");
    let no_commands = tempdir().expect("create isolated PATH");

    let output = devdock()
        .args(["cleanup"])
        .current_dir(workspace.path())
        .env("HOME", home.path())
        .env("DEVDOCK_ROOT", workspace.path())
        // Ensure this test exercises the missing-Docker-CLI path, regardless of
        // whether Docker is installed on the machine running the test.
        .env("PATH", no_commands.path())
        .output()
        .expect("run cleanup without docker");

    assert!(
        output.status.success(),
        "cleanup should not require Docker:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Docker is unavailable"),
        "cleanup should explain skipped Docker work:\n{stdout}"
    );
    assert!(stdout.contains("Cleanup Complete"), "cleanup should finish:\n{stdout}");
}

#[test]
fn guide_help_lists_intent_entrypoint() {
    let output = devdock().args(["guide", "--help"]).output().expect("run devdock guide --help");

    assert!(output.status.success(), "guide --help failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("intent"), "guide help should mention intent:\n{stdout}");
    assert!(
        stdout.contains("remote-start"),
        "guide help should list remote-start intent:\n{stdout}"
    );
    assert!(stdout.contains("add-service"), "guide help should list add-service intent:\n{stdout}");
}
