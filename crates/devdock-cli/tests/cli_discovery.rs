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
        stdout.contains("--copy-git-creds"),
        "start help should expose opt-in GitHub credential copy:\n{stdout}"
    );
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
fn recover_help_exposes_workspace_addressable_session_attachment() {
    let output = devdock().args(["recover", "--help"]).output().expect("run recover --help");

    assert!(output.status.success(), "recover --help failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--workspace"), "recover should require a workspace:\n{stdout}");
    assert!(
        stdout.contains("--from-workspace"),
        "recover should expose the bounded switcher handoff hook:\n{stdout}"
    );
}

#[test]
fn ports_forward_help_exposes_workspace_addressing() {
    let output =
        devdock().args(["ports", "forward", "--help"]).output().expect("run ports forward --help");

    assert!(output.status.success(), "ports forward --help failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--workspace <WORKSPACE>"),
        "ports forward should resolve by Coder workspace:\n{stdout}"
    );
}

#[test]
fn copy_git_creds_is_rejected_in_local_mode() {
    let output = devdock()
        .args(["start", "example-profile", "--copy-git-creds", "--dry-run"])
        .output()
        .expect("run local start with remote-only credential flag");

    assert!(!output.status.success(), "remote-only flag should fail in local mode");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--copy-git-creds requires remote mode"),
        "unexpected stderr:\n{stderr}"
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

#[test]
fn remote_start_guide_documents_git_credential_opt_in() {
    let output =
        devdock().args(["guide", "remote-start"]).output().expect("run remote start guide");

    assert!(output.status.success(), "remote start guide failed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--copy-git-creds"),
        "remote start guide should document credential opt-in:\n{stdout}"
    );
    assert!(
        stdout.contains("default automation credentials"),
        "remote start guide should document unchanged default behavior:\n{stdout}"
    );
}

#[test]
fn monitor_exposes_explicit_loopback_tilt_endpoint() {
    let help = devdock().args(["monitor", "--help"]).output().expect("run monitor help");
    assert!(help.status.success());
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("--tilt-endpoint"), "monitor help should expose endpoint:\n{stdout}");
    assert!(
        stdout.contains("http://127.0.0.1:10350"),
        "monitor help should show the local default:\n{stdout}"
    );

    let invalid = devdock()
        .args(["monitor", "--tilt-endpoint", "http://0.0.0.0:10350"])
        .output()
        .expect("run monitor with unsafe endpoint");
    assert!(!invalid.status.success(), "non-loopback endpoint should fail");
    let stderr = String::from_utf8_lossy(&invalid.stderr);
    assert!(
        stderr.contains("Tilt endpoint must use http://127.0.0.1"),
        "unexpected endpoint validation error:\n{stderr}"
    );
}
