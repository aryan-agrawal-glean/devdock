//! Configuration file synchronization to remote workspaces.
//!
//! DevDock transfers only ordinary configuration and optional GitHub
//! credentials. Gcloud credential transport is delegated to Scio's pinned,
//! gcloud-only helper and then verified separately.

use crate::{process::output_with_timeout, ssh, RemoteError, Result};
use devdock_core::profile::Profile;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tracing::{debug, info};

/// Pinned Scio helper contract approved for this engine-only integration.
const SCIO_CODER_SYNC_CREDS_SHA256: &str =
    "ee224f2c2391108097f748189066b237f0f3b59b85d4cadb6d282de9f2602bee";

// ─── Profile helpers (local only, no SSH) ──────────────────────────────────────

/// Get profile from YAML file.
fn get_profile(profile_path: &Path) -> Result<Profile> {
    let content = std::fs::read_to_string(profile_path)
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to read profile: {e}")))?;

    let profile: Profile = serde_yaml::from_str(&content)
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to parse profile: {e}")))?;

    Ok(profile)
}

/// Get platform from profile YAML.
pub fn get_profile_platform(profile_path: &Path) -> Result<String> {
    let profile = get_profile(profile_path)?;
    Ok(profile.platform.to_string())
}

/// Compute the remote destination path for a profile file.
/// Returns (remote_path, remote_dir).
fn compute_remote_profile_path(profile_path: &Path) -> Result<(String, String)> {
    let devdock_root = std::env::var("DEVDOCK_ROOT")
        .or_else(|_| std::env::var("PWD"))
        .unwrap_or_else(|_| ".".to_string());

    let workspace_root = std::path::PathBuf::from(&devdock_root);
    let profiles_base = workspace_root.join("devdock/profiles");

    let relative_path = if profile_path.starts_with(&profiles_base) {
        profile_path
            .strip_prefix(&profiles_base)
            .unwrap_or(profile_path)
            .to_string_lossy()
            .to_string()
    } else {
        profile_path
            .file_name()
            .ok_or_else(|| RemoteError::ConfigSyncError("Invalid profile path".to_string()))?
            .to_string_lossy()
            .to_string()
    };

    let remote_path = format!("/home/workspace/scio/devdock/profiles/{relative_path}");
    let remote_dir = if let Some(parent_idx) = remote_path.rfind('/') {
        remote_path[..parent_idx].to_string()
    } else {
        "/home/workspace/scio/devdock/profiles".to_string()
    };

    Ok((remote_path, remote_dir))
}

/// Return the selected profile's repository-relative path in the Coder Scio checkout.
///
/// Remote workspace reuse may reconcile only this DevDock-managed file before
/// converging the whole checkout to its requested immutable origin OID.
pub fn remote_profile_repo_path(profile_path: &Path) -> Result<String> {
    let (remote_path, _) = compute_remote_profile_path(profile_path)?;
    remote_path.strip_prefix("/home/workspace/scio/").map(str::to_string).ok_or_else(|| {
        RemoteError::ConfigSyncError(format!(
            "Selected profile path is outside the remote Scio checkout: {remote_path}"
        ))
    })
}

/// Sync only the selected profile file to a remote workspace.
pub fn sync_profile_file(workspace: &str, profile_path: &Path, verbose: bool) -> Result<()> {
    if !profile_path.exists() {
        return Err(RemoteError::ConfigSyncError(format!("Profile not found: {profile_path:?}")));
    }

    let profile_content = std::fs::read_to_string(profile_path)
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to read profile: {e}")))?;
    let (remote_profile_path, remote_profile_dir) = compute_remote_profile_path(profile_path)?;

    if verbose {
        println!("    → Syncing profile file to remote...");
    }

    let mut script = format!("set -e\nmkdir -p {}\n", shell_quote(&remote_profile_dir));
    script.push_str(&exact_text_write_command(&remote_profile_path, &profile_content));

    let mut command = Command::new("coder");
    command.arg("ssh").arg(workspace).arg("--").arg(&script);
    let output = output_with_timeout(&mut command, "coder ssh profile sync")
        .map_err(|e| RemoteError::ConfigSyncError(format!("Profile sync SSH failed: {e}")))?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RemoteError::ConfigSyncError(format!(
            "Failed to sync profile file:\nstdout:\n{stdout}\nstderr:\n{stderr}"
        )));
    }

    if verbose {
        println!("      ✓ Profile synced to {remote_profile_path}");
    }

    Ok(())
}

// ─── AWS credentials (local validation only, no SSH) ───────────────────────────

/// Validate AWS credentials via environment variables.
///
/// For AWS profiles, users must set these environment variables:
/// - AWS_ACCESS_KEY_ID (required)
/// - AWS_SECRET_ACCESS_KEY (required)
/// - AWS_SESSION_TOKEN (optional, for temporary credentials)
/// - AWS_REGION (required)
///
/// These will be automatically exported to the remote session.
pub fn sync_aws_credentials(_workspace: &str, verbose: bool) -> Result<()> {
    info!("Validating AWS credentials");
    if verbose {
        println!("    → Validating AWS environment variables...");
    }

    let has_access_key = std::env::var("AWS_ACCESS_KEY_ID").is_ok();
    let has_secret_key = std::env::var("AWS_SECRET_ACCESS_KEY").is_ok();
    let has_region =
        std::env::var("AWS_REGION").is_ok() || std::env::var("AWS_DEFAULT_REGION").is_ok();

    if !has_access_key || !has_secret_key {
        return Err(RemoteError::ConfigSyncError(
            "AWS credentials not found in environment variables.\n\n\
             AWS profiles require these environment variables to be set:\n\n\
             Required:\n\
               export AWS_ACCESS_KEY_ID=your_access_key_id\n\
               export AWS_SECRET_ACCESS_KEY=your_secret_access_key\n\
               export AWS_REGION=us-east-1\n\n\
             Optional (for temporary credentials):\n\
               export AWS_SESSION_TOKEN=your_session_token\n\n\
             These variables will be automatically exported to the remote workspace.\n"
                .to_string(),
        ));
    }

    if !has_region {
        eprintln!("\n⚠️  Warning: AWS_REGION not set");
        eprintln!("   Services may default to us-east-1 or fail");
        eprintln!("   Set region: export AWS_REGION=us-east-1\n");
    }

    if verbose {
        println!("      ✓ AWS_ACCESS_KEY_ID: set");
        println!("      ✓ AWS_SECRET_ACCESS_KEY: set");
        if std::env::var("AWS_SESSION_TOKEN").is_ok() {
            println!("      ✓ AWS_SESSION_TOKEN: set");
        }
        if let Ok(region) =
            std::env::var("AWS_REGION").or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
        {
            println!("      ✓ AWS_REGION: {}", region);
        }
        println!("    ✓ AWS credentials validated (will be exported to remote)");
    }

    Ok(())
}

// ─── GitHub credential staging and sync ───────────────────────────────────────

#[derive(Serialize)]
struct GhHosts {
    #[serde(rename = "github.com")]
    github_com: GhHost,
}

#[derive(Serialize)]
struct GhHost {
    git_protocol: &'static str,
    users: BTreeMap<String, GhUser>,
    user: String,
}

#[derive(Serialize)]
struct GhUser {
    oauth_token: String,
}

fn local_gh_output(args: &[&str], description: &str) -> Result<String> {
    let output = Command::new("gh").args(args).output().map_err(|e| {
        RemoteError::ConfigSyncError(format!("Failed to run local gh CLI ({description}): {e}"))
    })?;

    if !output.status.success() {
        return Err(RemoteError::ConfigSyncError(format!(
            "Local GitHub authentication is unavailable ({description}). \
             Run `gh auth login --hostname github.com` and retry with --copy-git-creds."
        )));
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        return Err(RemoteError::ConfigSyncError(format!(
            "Local GitHub authentication returned no {description}. \
             Run `gh auth login --hostname github.com` and retry with --copy-git-creds."
        )));
    }
    Ok(value)
}

fn read_local_git_credentials() -> Result<(String, String)> {
    which::which("git").map_err(|_| {
        RemoteError::ConfigSyncError(
            "git CLI not found; install git before using --copy-git-creds.".to_string(),
        )
    })?;
    which::which("gh").map_err(|_| {
        RemoteError::ConfigSyncError(
            "GitHub CLI not found; install gh and run `gh auth login --hostname github.com` \
             before using --copy-git-creds."
                .to_string(),
        )
    })?;

    let token =
        local_gh_output(&["auth", "token", "--hostname", "github.com"], "authentication token")?;
    let username = local_gh_output(
        &["api", "--hostname", "github.com", "user", "--jq", ".login"],
        "username",
    )?;
    Ok((username, token))
}

/// Fail fast before any remote workspace mutation when local GitHub credentials cannot be copied.
pub fn check_local_git_credentials() -> Result<()> {
    let _ = read_local_git_credentials()?;
    Ok(())
}

/// Build a minimal github.com-only gh config in a private temporary directory.
fn stage_git_credentials_locally() -> Result<tempfile::TempDir> {
    let (username, token) = read_local_git_credentials()?;
    let temp_dir = tempfile::tempdir().map_err(|e| {
        RemoteError::ConfigSyncError(format!("Failed to stage GitHub credentials: {e}"))
    })?;
    write_staged_git_credentials(temp_dir.path(), &username, &token)?;
    Ok(temp_dir)
}

fn write_staged_git_credentials(
    staging_root: &Path,
    username: &str,
    token: &str,
) -> Result<PathBuf> {
    let gh_dir = staging_root.join("git-creds/gh");
    std::fs::create_dir_all(&gh_dir).map_err(|e| {
        RemoteError::ConfigSyncError(format!("Failed to stage GitHub credentials: {e}"))
    })?;

    let mut users = BTreeMap::new();
    users.insert(username.to_string(), GhUser { oauth_token: token.to_string() });
    let hosts =
        GhHosts { github_com: GhHost { git_protocol: "https", users, user: username.to_string() } };
    let hosts_yaml = serde_yaml::to_string(&hosts).map_err(|e| {
        RemoteError::ConfigSyncError(format!("Failed to serialize GitHub credentials: {e}"))
    })?;
    let hosts_path = gh_dir.join("hosts.yml");
    std::fs::write(&hosts_path, hosts_yaml).map_err(|e| {
        RemoteError::ConfigSyncError(format!("Failed to stage GitHub credentials: {e}"))
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hosts_path, std::fs::Permissions::from_mode(0o600)).map_err(
            |e| RemoteError::ConfigSyncError(format!("Failed to secure staged credentials: {e}")),
        )?;
    }

    Ok(hosts_path)
}

fn scp_git_credentials_to_remote(workspace: &str, temp_dir: &Path) -> Result<()> {
    let source = temp_dir.join("git-creds");
    let mut command = git_credentials_scp_command(workspace, &source);
    let output = output_with_timeout(&mut command, "scp GitHub credentials").map_err(|e| {
        RemoteError::ConfigSyncError(format!("Failed to transfer GitHub credentials: {e}"))
    })?;

    if !output.status.success() {
        return Err(RemoteError::ConfigSyncError(
            "Failed to transfer GitHub credentials with scp; credential data was not included in the error output."
                .to_string(),
        ));
    }
    Ok(())
}

fn git_credentials_scp_command(workspace: &str, source: &Path) -> Command {
    let target = format!("{workspace}.coder:~/.secrets-sync/");
    let mut command = Command::new("scp");
    command
        .arg("-O")
        .arg("-q")
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("UserKnownHostsFile=/dev/null")
        .arg("-o")
        .arg("ConnectTimeout=30")
        .arg("-r")
        .arg(source)
        .arg(target);
    command
}

fn git_credentials_finalize_script() -> &'static str {
    r#"set -e

test -s ~/.secrets-sync/git-creds/gh/hosts.yml

# An empty user-level helper resets lower-priority helpers, including the system App helper.
git config --global --unset-all credential.https://github.com.helper 2>/dev/null || true
git config --global --add credential.https://github.com.helper ''
git config --global --add credential.https://github.com.helper \
  '!env -u GH_TOKEN -u GITHUB_TOKEN gh auth git-credential'
git config --global credential.https://github.com.useHttpPath false

# The cache may be root-owned and refreshed by the template. Remove it when permitted; the
# empty helper reset and environment-stripping helper ensure it is ignored either way.
sudo -n rm -f /run/glean/github-token 2>/dev/null || true

mkdir -p ~/.config/gh ~/.config/devdock
chmod 700 ~/.config/gh ~/.config/devdock
install -m 600 ~/.secrets-sync/git-creds/gh/hosts.yml ~/.config/gh/hosts.yml

cat > ~/.config/devdock/user-git-env.sh <<'DEVDOCK_GIT_ENV_EOF'
unset GH_TOKEN
unset GITHUB_TOKEN
DEVDOCK_GIT_ENV_EOF
chmod 600 ~/.config/devdock/user-git-env.sh

for shell_file in ~/.profile ~/.bashrc; do
  touch "$shell_file"
  if ! grep -q 'DEVDOCK_USER_GIT_CREDS' "$shell_file"; then
    printf '\n# DEVDOCK_USER_GIT_CREDS\n[ ! -f "$HOME/.config/devdock/user-git-env.sh" ] || . "$HOME/.config/devdock/user-git-env.sh"\n' >> "$shell_file"
  fi
done

rm -rf ~/.secrets-sync/git-creds

test "$(stat -c '%a' ~/.config/gh/hosts.yml)" = "600"
test "$(git config --global --get-all credential.https://github.com.helper | sed -n '1p')" = ""

# Verify the configured Git credential path directly. Avoid `gh auth status`, which can trigger
# keyring migration and require desktop-only dependencies such as dbus-launch on remote hosts.
printf 'protocol=https\nhost=github.com\n\n' |
  env -u GH_TOKEN -u GITHUB_TOKEN git credential fill 2>/dev/null |
  awk -F= '
    $1 == "username" && length($2) { username = 1 }
    $1 == "password" && length($2) { password = 1 }
    END { exit !(username && password) }
  '
echo "DEVDOCK_GIT_CREDS:configured"
"#
}

fn finalize_git_credentials(workspace: &str, log_file: Option<&Path>, verbose: bool) -> Result<()> {
    let output =
        crate::ssh::ssh_exec_with_log(workspace, git_credentials_finalize_script(), log_file)?;
    if !output.lines().any(|line| line == "DEVDOCK_GIT_CREDS:configured") {
        return Err(RemoteError::ConfigSyncError(
            "GitHub credentials were transferred but remote verification failed; \
             bot credentials were not retained as a fallback."
                .to_string(),
        ));
    }
    if verbose {
        println!("      ✓ User GitHub credentials installed (hosts.yml mode 600)");
        println!("      ✓ System GitHub App credential helper overridden for this user");
    }
    Ok(())
}

fn git_credentials_staging_cleanup_script() -> &'static str {
    r#"set -e
rm -rf ~/.secrets-sync/git-creds
"#
}

fn cleanup_remote_git_credential_staging(workspace: &str, log_file: Option<&Path>) -> Result<()> {
    crate::ssh::ssh_exec_with_log(workspace, git_credentials_staging_cleanup_script(), log_file)?;
    Ok(())
}

fn sync_staged_git_credentials(
    workspace: &str,
    temp_dir: &Path,
    log_file: Option<&Path>,
    verbose: bool,
) -> Result<()> {
    let transfer_result = scp_git_credentials_to_remote(workspace, temp_dir)
        .and_then(|_| finalize_git_credentials(workspace, log_file, verbose));

    if let Err(error) = transfer_result {
        if cleanup_remote_git_credential_staging(workspace, log_file).is_err() {
            return Err(RemoteError::ConfigSyncError(
                "GitHub credential transfer failed and its remote temporary staging directory could not be removed. Inspect the owned workspace before retrying."
                    .to_string(),
            ));
        }
        return Err(error);
    }

    Ok(())
}

// ─── Main entry point ──────────────────────────────────────────────────────────

/// Sync profile/custom.ini and optional GitHub credentials to a remote workspace.
///
/// Gcloud credentials are deliberately excluded: remote startup must use
/// `sync_gcloud_credentials_via_scio_helper` as its sole credential transport.
///
pub fn sync_all_config(
    workspace: &str,
    profile_path: &Path,
    custom_ini_path: Option<&Path>,
    copy_git_creds: bool,
    log_file: Option<&Path>,
    verbose: bool,
) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;

    // Log start of config sync
    if let Some(log_path) = log_file {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
            let _ = writeln!(file, "\n=== Configuration Sync ===");
            let _ = writeln!(file, "Workspace: {workspace}");
            let _ = writeln!(file, "Profile: {}", profile_path.display());
        }
    }

    // ─── Local preparation (no SSH) ────────────────────────────────────────

    if !profile_path.exists() {
        return Err(RemoteError::ConfigSyncError(format!("Profile not found: {profile_path:?}")));
    }
    let profile_content = std::fs::read_to_string(profile_path)
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to read profile: {e}")))?;
    let (remote_profile_path, remote_profile_dir) = compute_remote_profile_path(profile_path)?;

    let custom_ini_content = if let Some(path) = custom_ini_path {
        if path.exists() {
            Some(std::fs::read_to_string(path).map_err(|e| {
                RemoteError::ConfigSyncError(format!("Failed to read custom.ini: {e}"))
            })?)
        } else {
            None
        }
    } else {
        None
    };

    let git_staging = if copy_git_creds { Some(stage_git_credentials_locally()?) } else { None };
    let needs_secrets_sync = git_staging.is_some();

    // ─── Phase 1: Single SSH for all config file writes ────────────────────

    if verbose {
        println!("    → Syncing config files to remote (single connection)...");
    }
    info!("Phase 1: Syncing config files to {} in single SSH", workspace);

    let mut script = String::new();
    script.push_str("set -e\n\n");

    // Create all directories in one shot
    if needs_secrets_sync {
        script.push_str("rm -rf ~/.secrets-sync\n");
    }
    script.push_str(&format!(
        "mkdir -p '{}' \
         /home/workspace/scio/devdock/profiles/user_profiles \
         /home/workspace/scio/devdock/profiles/integration_tests \
         /home/workspace/scio/config/{}\n\n",
        remote_profile_dir,
        if needs_secrets_sync { " ~/.secrets-sync" } else { "" }
    ));

    // Preserve the local file byte-for-byte. A heredoc would add a newline when
    // the source already ends with one, recreating managed profile residue.
    script.push_str(&exact_text_write_command(&remote_profile_path, &profile_content));
    script.push('\n');

    // Write custom.ini via heredoc (conditional)
    if let Some(ref ini_content) = custom_ini_content {
        script.push_str(&exact_text_write_command(
            "/home/workspace/scio/config/custom.ini",
            ini_content,
        ));
        script.push('\n');
    }

    // Merge Claude MCP config via Python (non-critical: || true so set -e doesn't abort)
    script
        .push_str("python3 << 'DEVDOCK_PYTHON_EOF' || echo 'DEVDOCK_WARN:claude_config_failed'\n");
    script.push_str(
        r#"import json
from pathlib import Path

devdock_mcp_config = {
    "devdock": {
        "type": "stdio",
        "command": "python",
        "args": ["/home/workspace/scio/devdock/core/mcp_server/main.py"],
        "description": "Devdock MCP server for devdock resource logs and error analysis"
    }
}

claude_config_path = Path.home() / ".claude.json"

if claude_config_path.exists():
    try:
        with open(claude_config_path, 'r') as f:
            config = json.load(f)
    except json.JSONDecodeError:
        config = {}
else:
    config = {}

if "mcpServers" not in config:
    config["mcpServers"] = {}

config["mcpServers"]["devdock"] = devdock_mcp_config["devdock"]

with open(claude_config_path, 'w') as f:
    json.dump(config, f, indent=2)

print(f"✓ Claude MCP configuration updated at {claude_config_path}")
"#,
    );
    script.push_str("DEVDOCK_PYTHON_EOF\n");

    let mut command = Command::new("coder");
    command.arg("ssh").arg(workspace).arg("--").arg(&script);
    let output = output_with_timeout(&mut command, "coder ssh configuration sync")
        .map_err(|e| RemoteError::ConfigSyncError(format!("Phase 1 SSH failed: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        eprintln!("\n❌ Failed to sync config files to remote");
        eprintln!("   Error: {}", stderr);
        if !stdout.is_empty() {
            eprintln!("   Output: {}", stdout);
        }
        return Err(RemoteError::ConfigSyncError(format!(
            "Phase 1 config sync failed: {}",
            stderr
        )));
    }

    // Check for non-critical warnings in output
    let claude_config_ok = !stdout.contains("DEVDOCK_WARN:claude_config_failed");
    if !claude_config_ok {
        debug!("Claude config merge failed (non-critical)");
        if verbose {
            eprintln!("    ⚠ Warning: Failed to sync Claude MCP config");
        }
        if let Some(log_path) = log_file {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
                let _ = writeln!(file, "WARNING: Failed to sync Claude config");
            }
        }
    }

    if verbose {
        println!("      ✓ Profile synced to {}", remote_profile_path);
        if custom_ini_content.is_some() {
            println!("      ✓ custom.ini synced");
        }
        if claude_config_ok {
            for line in stdout.lines() {
                if line.contains("Claude MCP") {
                    println!("      {}", line.trim());
                }
            }
        }
    }

    debug!("Phase 1 complete: config files synced");

    // ─── Optional GitHub credential sync (gcloud is handled by Scio) ────────
    if let Some(ref temp_dir) = git_staging {
        if verbose {
            println!("    → Copying local GitHub credentials...");
        }
        sync_staged_git_credentials(workspace, temp_dir.path(), log_file, verbose)?;
    }

    match get_profile_platform(profile_path).ok().as_deref() {
        Some("aws") => {
            if let Err(e) = sync_aws_credentials(workspace, verbose) {
                return Err(RemoteError::ConfigSyncError(format!(
                    "Failed to sync AWS credentials (required for platform 'aws'): {e}"
                )));
            }
        }
        Some("azure") | Some("google") | Some("gcp") => {
            if verbose {
                println!("    → Platform credentials handled outside DevDock config sync");
            }
        }
        Some(other) => {
            if verbose {
                println!("    → Unknown platform '{}', skipping credential sync", other);
            }
        }
        None => {
            if verbose {
                println!("    → Could not determine platform, skipping platform credential sync");
            }
        }
    }

    if let Some(log_path) = log_file {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
            let _ = writeln!(file, "✓ Configuration sync completed");
        }
    }

    if verbose {
        println!("    ✓ All configuration files synced");
    }

    Ok(())
}

/// Invoke Scio's checked-out credential transport in its gcloud-only mode.
///
/// DevDock owns sequencing and verification, while Scio owns credential
/// selection, filtering, and transport. Helper stdout/stderr are deliberately
/// not copied into operation logs because they may contain credential-adjacent
/// information or obsolete remote-login guidance.
pub fn sync_gcloud_credentials_via_scio_helper(
    workspace: &str,
    project: &str,
    log_file: Option<&Path>,
    verbose: bool,
) -> Result<()> {
    let helper = resolve_scio_credential_helper()?;
    info!(workspace, helper = %helper.display(), "Invoking Scio gcloud credential helper");
    if verbose {
        println!("    → Syncing gcloud credentials through the Scio helper...");
    }

    let mut command = Command::new(&helper);
    command
        .arg(workspace)
        .args(scio_gcloud_helper_args())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = output_with_timeout(&mut command, "Scio gcloud credential helper")
        .map_err(|error| {
            RemoteError::ConfigSyncError(format!(
                "Failed to start Scio gcloud credential helper: {error}"
            ))
        })?
        .status;
    if !status.success() {
        return Err(RemoteError::ConfigSyncError(format!(
            "Scio gcloud credential helper failed with exit status {}",
            status.code().map_or_else(|| "signal".to_string(), |code| code.to_string())
        )));
    }

    verify_remote_gcloud_project(workspace, project, log_file)?;
    if verbose {
        println!("      ✓ Remote user auth, ADC, and selected project verified");
    }
    Ok(())
}

fn resolve_scio_credential_helper() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("DEVDOCK_CODER_SYNC_CREDS") {
        return require_scio_credential_helper(PathBuf::from(path));
    }

    let mut roots = Vec::new();
    for variable in ["DEVDOCK_SCIO_ROOT", "DEVDOCK_ROOT"] {
        if let Some(root) = std::env::var_os(variable) {
            roots.push(PathBuf::from(root));
        }
    }
    if let Ok(current_dir) = std::env::current_dir() {
        roots.push(current_dir);
    }

    for root in roots {
        let candidate = root.join("coder/coder-sync-creds.sh");
        if candidate.is_file() {
            return require_scio_credential_helper(candidate);
        }
    }

    Err(RemoteError::ConfigSyncError(
        "Scio credential helper not found. Run from the Scio checkout, set DEVDOCK_SCIO_ROOT, or set DEVDOCK_CODER_SYNC_CREDS to coder/coder-sync-creds.sh. The legacy DevDock gcloud-copy path is intentionally unavailable."
            .to_string(),
    ))
}

fn require_scio_credential_helper(path: PathBuf) -> Result<PathBuf> {
    if !path.is_file() {
        Err(RemoteError::ConfigSyncError(format!(
            "DEVDOCK_CODER_SYNC_CREDS does not point to a file: {}",
            path.display()
        )))
    } else if !scio_helper_has_expected_checksum(&path)? {
        Err(RemoteError::ConfigSyncError(format!(
            "Scio credential helper checksum does not match the supported contract at {}. Update the helper integration or use the pinned Scio checkout; DevDock will not fall back to its legacy gcloud copy path.",
            path.display()
        )))
    } else {
        Ok(path)
    }
}

fn scio_helper_has_expected_checksum(path: &Path) -> Result<bool> {
    let bytes = std::fs::read(path).map_err(|error| {
        RemoteError::ConfigSyncError(format!(
            "Failed to read Scio credential helper {}: {error}",
            path.display()
        ))
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)) == SCIO_CODER_SYNC_CREDS_SHA256)
}

fn verify_remote_gcloud_project(
    workspace: &str,
    project: &str,
    log_file: Option<&Path>,
) -> Result<()> {
    let project = project.trim();
    if project.is_empty() {
        return Err(RemoteError::ConfigSyncError(
            "selected profile has no usable project".to_string(),
        ));
    }

    let script = remote_gcloud_verification_script(project);
    let output = ssh::ssh_exec_with_log(workspace, &script, log_file)?;
    if !output.lines().any(|line| line == "DEVDOCK_REMOTE_GCLOUD_OK") {
        return Err(RemoteError::ConfigSyncError(
            "remote gcloud verification did not report success".to_string(),
        ));
    }
    Ok(())
}

fn scio_gcloud_helper_args() -> [&'static str; 5] {
    [
        "--skip-glean-token",
        "--skip-glean-mcp",
        "--skip-gpg",
        "--skip-codex-install",
        "--skip-remote-control",
    ]
}

fn remote_gcloud_verification_script(project: &str) -> String {
    let project = shell_quote(project);
    format!(
        r#"set -eu
project={project}
gcloud config set project "$project" >/dev/null 2>&1
test "$(gcloud config get-value project 2>/dev/null)" = "$project"
gcloud auth print-access-token >/dev/null 2>&1
gcloud auth application-default print-access-token >/dev/null 2>&1
gcloud projects describe "$project" --format='value(projectNumber)' >/dev/null 2>&1
printf 'DEVDOCK_REMOTE_GCLOUD_OK\n'
"#
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn exact_text_write_command(path: &str, content: &str) -> String {
    format!("printf '%s' {} > {}\n", shell_quote(content), shell_quote(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_credentials_finalize_clears_bot_helper_before_installing_user_auth() {
        let script = git_credentials_finalize_script();
        let reset = script
            .find("git config --global --add credential.https://github.com.helper ''")
            .expect("empty helper reset");
        let user_helper = script
            .find("!env -u GH_TOKEN -u GITHUB_TOKEN gh auth git-credential")
            .expect("user gh helper");
        let install = script.find("install -m 600").expect("credential install");

        assert!(reset < user_helper, "system helper must be reset before adding the user helper");
        assert!(user_helper < install, "helper must be cleared before credentials are installed");
        assert!(script.contains("rm -f /run/glean/github-token"));
        assert!(script.contains("unset GH_TOKEN"));
        assert!(script.contains("git credential fill"));
        assert!(script.contains("protocol=https\\nhost=github.com\\n\\n"));
        assert!(!script.contains("gh auth status --hostname github.com"));
    }

    #[test]
    fn git_credentials_finalize_never_embeds_a_token() {
        let script = git_credentials_finalize_script();

        assert!(!script.contains("oauth_token:"));
        assert!(!script.contains("gh auth token"));
        assert!(!script.contains("auth login"));
        assert!(script.contains("~/.secrets-sync/git-creds/gh/hosts.yml"));
    }

    #[test]
    fn staged_git_credentials_are_github_com_only_and_private() {
        let staging = tempfile::tempdir().expect("create credential staging directory");
        let hosts_path = write_staged_git_credentials(staging.path(), "octocat", "test-token")
            .expect("stage minimal GitHub credentials");
        let hosts: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&hosts_path).expect("read staged hosts"))
                .expect("parse staged hosts");
        let hosts = hosts.as_mapping().expect("hosts mapping");

        assert_eq!(hosts.len(), 1);
        assert!(hosts.contains_key("github.com"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(hosts_path).expect("staged metadata").permissions().mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn git_credential_transfer_uses_legacy_scp_and_scoped_failure_cleanup() {
        let command = git_credentials_scp_command("owner-workspace", Path::new("/run/git-creds"));
        let args =
            command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect::<Vec<_>>();

        assert!(args.contains(&"-O".to_string()));
        assert!(args.contains(&"StrictHostKeyChecking=no".to_string()));
        assert_eq!(args.last(), Some(&"owner-workspace.coder:~/.secrets-sync/".to_string()));

        let cleanup = git_credentials_staging_cleanup_script();
        assert!(cleanup.contains("rm -rf ~/.secrets-sync/git-creds"));
        assert!(!cleanup.contains("~/.config/gh"));
        assert!(!cleanup.contains("auth login"));
    }

    #[test]
    fn scio_helper_is_invoked_in_gcloud_only_mode() {
        assert_eq!(
            scio_gcloud_helper_args(),
            [
                "--skip-glean-token",
                "--skip-glean-mcp",
                "--skip-gpg",
                "--skip-codex-install",
                "--skip-remote-control",
            ]
        );
    }

    #[test]
    fn remote_gcloud_verification_is_status_only_and_never_interactive() {
        let script = remote_gcloud_verification_script("profile-project");

        assert!(script.contains("gcloud config set project"));
        assert!(script.contains("gcloud auth print-access-token >/dev/null 2>&1"));
        assert!(
            script.contains("gcloud auth application-default print-access-token >/dev/null 2>&1")
        );
        assert!(script.contains("gcloud projects describe"));
        assert!(!script.contains("gcloud auth login"));
        assert!(!script.contains("application-default login"));
    }

    #[test]
    fn exact_text_write_command_preserves_profile_bytes() {
        let temp_dir = tempfile::tempdir().expect("create output directory");
        let output_path = temp_dir.path().join("profile.yaml");
        let content = "project: qe_smoke_test\n";
        let script = format!(
            "set -e\n{}",
            exact_text_write_command(&output_path.display().to_string(), content)
        );
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .output()
            .expect("run exact write script");

        assert!(output.status.success());
        assert_eq!(std::fs::read(output_path).expect("read output"), content.as_bytes());
    }
}
