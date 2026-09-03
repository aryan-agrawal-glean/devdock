//! Configuration file synchronization to remote workspaces.
//!
//! Optimized for minimal SSH connections:
//! - Phase 1 (1 SSH): Create directories, write profile, custom.ini, and claude.json
//! - Phase 2 (1 SCP): Copy gcloud credential files (GCP/unknown platform only)
//! - Phase 3 (1 SSH): Finalize gcloud - move creds, verify auth, set project (GCP/unknown platform only)

use crate::{RemoteError, Result};
use devdock_core::profile::Profile;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use tracing::{debug, info};

// ─── Profile helpers (local only, no SSH) ──────────────────────────────────────

/// Get profile from YAML file.
fn get_profile(profile_path: &Path) -> Result<Profile> {
    let content = std::fs::read_to_string(profile_path)
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to read profile: {e}")))?;

    let profile: Profile = serde_yaml::from_str(&content)
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to parse profile: {e}")))?;

    Ok(profile)
}

/// Get GCP project from profile YAML.
pub fn get_profile_project(profile_path: &Path) -> Result<String> {
    let profile = get_profile(profile_path)?;
    Ok(profile.project)
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

    let script = format!(
        "set -e\nmkdir -p '{}'\ncat > '{}' << 'DEVDOCK_PROFILE_EOF'\n{}\nDEVDOCK_PROFILE_EOF\n",
        remote_profile_dir, remote_profile_path, profile_content
    );

    let output = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg(&script)
        .output()
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

// ─── GCloud credential staging (local + SCP, no coder ssh) ────────────────────

/// Stage gcloud credentials locally into a temp directory for SCP transfer.
/// Returns None if no gcloud credentials exist locally.
/// Caller must keep the returned TempDir alive until SCP completes.
fn stage_gcloud_locally(verbose: bool) -> Result<Option<(tempfile::TempDir, usize)>> {
    let gcloud_dir = dirs::home_dir()
        .ok_or_else(|| RemoteError::ConfigSyncError("Cannot determine home directory".to_string()))?
        .join(".config/gcloud");

    if !gcloud_dir.exists() {
        debug!("No ~/.config/gcloud found; skipping gcloud sync");
        if verbose {
            println!("    → No local gcloud credentials found, skipping");
        }
        return Ok(None);
    }

    let temp_dir = tempfile::tempdir().map_err(|e| {
        RemoteError::ConfigSyncError(format!("Failed to create temp directory: {e}"))
    })?;
    let gcloud_temp = temp_dir.path().join("gcloud");

    std::fs::create_dir_all(&gcloud_temp).map_err(|e| {
        RemoteError::ConfigSyncError(format!("Failed to create gcloud temp dir: {e}"))
    })?;
    std::fs::create_dir_all(gcloud_temp.join("configurations")).map_err(|e| {
        RemoteError::ConfigSyncError(format!("Failed to create configurations dir: {e}"))
    })?;

    let gcloud_files = vec![
        "application_default_credentials.json",
        "access_tokens.db",
        "credentials.db",
        "active_config",
        "configurations/config_default",
        "env.yaml",
    ];

    info!("Staging gcloud credential files...");
    let mut staged_count = 0usize;

    for file in &gcloud_files {
        let src = gcloud_dir.join(file);
        if src.exists() {
            let dest = gcloud_temp.join(file);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    RemoteError::ConfigSyncError(format!("Failed to create parent dir: {e}"))
                })?;
            }
            std::fs::copy(&src, &dest)
                .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to copy {file}: {e}")))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&dest)
                    .map_err(|e| {
                        RemoteError::ConfigSyncError(format!("Failed to get metadata: {e}"))
                    })?
                    .permissions();
                perms.set_mode(0o600);
                std::fs::set_permissions(&dest, perms).map_err(|e| {
                    RemoteError::ConfigSyncError(format!("Failed to set permissions: {e}"))
                })?;
            }

            debug!("Staged file: {}", file);
            staged_count += 1;
        }
    }

    if !gcloud_temp.join("application_default_credentials.json").exists() {
        return Err(RemoteError::ConfigSyncError(
            "application_default_credentials.json not found locally - run: gcloud auth application-default login".to_string()
        ));
    }

    if verbose {
        println!("    → Staged {} credential files", staged_files_msg(staged_count));
    }
    info!("Staged {} gcloud files", staged_count);

    Ok(Some((temp_dir, staged_count)))
}

fn staged_files_msg(count: usize) -> String {
    format!("{count} credential file{}", if count == 1 { "" } else { "s" })
}

/// SCP staged gcloud credentials to remote workspace.
fn scp_gcloud_to_remote(workspace: &str, temp_dir: &Path, verbose: bool) -> Result<()> {
    let gcloud_temp = temp_dir.join("gcloud");
    let gcloud_temp_str = gcloud_temp.to_string_lossy();
    let scp_target = format!("{workspace}.coder:~/.secrets-sync/");

    info!("Copying gcloud credentials to remote...");
    if verbose {
        println!("      (copying files via scp...)");
    }

    let output = Command::new("scp")
        .arg("-O")
        .arg("-q")
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("UserKnownHostsFile=/dev/null")
        .arg("-o")
        .arg("ConnectTimeout=30")
        .arg("-o")
        .arg("ServerAliveInterval=10")
        .arg("-o")
        .arg("ServerAliveCountMax=3")
        .arg("-r")
        .arg(gcloud_temp_str.as_ref())
        .arg(&scp_target)
        .output()
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to run scp: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RemoteError::ConfigSyncError(format!(
            "Failed to copy gcloud credentials: {stderr}"
        )));
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

/// Fail fast before workspace creation when local GitHub credentials cannot be copied.
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
    let gh_dir = temp_dir.path().join("git-creds/gh");
    std::fs::create_dir_all(&gh_dir).map_err(|e| {
        RemoteError::ConfigSyncError(format!("Failed to stage GitHub credentials: {e}"))
    })?;

    let mut users = BTreeMap::new();
    users.insert(username.clone(), GhUser { oauth_token: token });
    let hosts = GhHosts { github_com: GhHost { git_protocol: "https", users, user: username } };
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

    Ok(temp_dir)
}

fn scp_git_credentials_to_remote(workspace: &str, temp_dir: &Path) -> Result<()> {
    let source = temp_dir.join("git-creds");
    let target = format!("{workspace}.coder:~/.secrets-sync/");
    let output = Command::new("scp")
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
        .arg(target)
        .output()
        .map_err(|e| {
            RemoteError::ConfigSyncError(format!("Failed to transfer GitHub credentials: {e}"))
        })?;

    if !output.status.success() {
        return Err(RemoteError::ConfigSyncError(format!(
            "Failed to transfer GitHub credentials: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
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

// ─── Main entry point ──────────────────────────────────────────────────────────

/// Status returned from config sync, including credential verification results.
/// This avoids the need for a separate SSH call to verify auth.
#[derive(Debug, Default)]
pub struct ConfigSyncStatus {
    /// Whether ADC credentials are expired on the remote
    pub adc_expired: bool,
}

/// Sync all configuration files to remote workspace.
///
/// Uses minimal SSH connections:
/// - Phase 1 (1 SSH): Create directories, write profile, custom.ini, and claude.json
/// - Phase 2 (1 SCP): Copy gcloud credentials (GCP/unknown platform only)
/// - Phase 3 (1 SSH): Finalize gcloud - move creds, verify auth, set project (GCP/unknown platform only)
///
/// Returns `ConfigSyncStatus` with credential verification results from Phase 3,
/// allowing the caller to handle ADC expiration without an additional SSH call.
pub fn sync_all_config(
    workspace: &str,
    profile_path: &Path,
    custom_ini_path: Option<&Path>,
    copy_git_creds: bool,
    log_file: Option<&Path>,
    verbose: bool,
) -> Result<ConfigSyncStatus> {
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

    let platform = get_profile_platform(profile_path).ok();
    let is_gcp = matches!(platform.as_deref(), Some("google") | Some("gcp"));

    // Stage gcloud credentials locally before any SSH (fail fast for GCP, best-effort for unknown)
    let gcloud_staging = if is_gcp {
        if verbose {
            println!("    → Syncing gcloud credentials (this may take 10-20s)...");
        }
        stage_gcloud_locally(verbose)?
    } else if platform.is_none() {
        if verbose {
            println!("    → Could not determine platform, checking gcloud credentials...");
        }
        match stage_gcloud_locally(verbose) {
            Ok(staging) => staging,
            Err(e) => {
                debug!("Best-effort gcloud staging failed: {}", e);
                None
            }
        }
    } else {
        None
    };

    let git_staging = if copy_git_creds { Some(stage_git_credentials_locally()?) } else { None };
    let needs_secrets_sync = gcloud_staging.is_some() || git_staging.is_some();

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

    // Write profile via heredoc
    script.push_str(&format!(
        "cat > '{}' << 'DEVDOCK_PROFILE_EOF'\n{}\nDEVDOCK_PROFILE_EOF\n\n",
        remote_profile_path, profile_content
    ));

    // Write custom.ini via heredoc (conditional)
    if let Some(ref ini_content) = custom_ini_content {
        script.push_str(&format!(
            "cat > /home/workspace/scio/config/custom.ini << 'DEVDOCK_INI_EOF'\n{}\nDEVDOCK_INI_EOF\n\n",
            ini_content
        ));
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

    let output = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg(&script)
        .output()
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

    // ─── Credential sync (Phase 2 + 3 for GCP, local-only for others) ─────

    let mut status = ConfigSyncStatus::default();

    if let Some(ref temp_dir) = git_staging {
        if verbose {
            println!("    → Copying local GitHub credentials...");
        }
        scp_git_credentials_to_remote(workspace, temp_dir.path())?;
        finalize_git_credentials(workspace, log_file, verbose)?;
    }

    match platform.as_deref() {
        Some("google") | Some("gcp") => {
            if let Some((ref temp_dir, _count)) = gcloud_staging {
                // Phase 2: SCP gcloud credentials
                scp_gcloud_to_remote(workspace, temp_dir.path(), verbose)?;

                // Phase 3: Single SSH for gcloud finalization
                let project = get_profile_project(profile_path).ok();
                status.adc_expired =
                    run_gcloud_finalize(workspace, project.as_deref(), log_file, verbose)?;
            }
        }
        Some("aws") => {
            if let Err(e) = sync_aws_credentials(workspace, verbose) {
                return Err(RemoteError::ConfigSyncError(format!(
                    "Failed to sync AWS credentials (required for platform 'aws'): {e}"
                )));
            }
        }
        Some("azure") => {
            if verbose {
                println!("    → Azure platform detected (credentials managed via az cli)");
            }
        }
        Some(other) => {
            if verbose {
                println!("    → Unknown platform '{}', skipping credential sync", other);
            }
        }
        None => {
            if verbose {
                println!("    → Could not determine platform, syncing available credentials...");
            }
            // Best-effort gcloud: SCP + finalize (swallow errors)
            if let Some((ref temp_dir, _count)) = gcloud_staging {
                if let Ok(()) = scp_gcloud_to_remote(workspace, temp_dir.path(), verbose) {
                    if let Ok(adc_expired) = run_gcloud_finalize(workspace, None, log_file, verbose)
                    {
                        status.adc_expired = adc_expired;
                    }
                }
            }
            let _ = sync_aws_credentials(workspace, verbose);
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

    Ok(status)
}

/// Phase 3: Finalize gcloud setup on remote in a single SSH connection.
///
/// Moves staged credentials to final locations, verifies auth, and optionally sets project.
/// Returns whether ADC credentials are expired.
fn run_gcloud_finalize(
    workspace: &str,
    project: Option<&str>,
    log_file: Option<&Path>,
    verbose: bool,
) -> Result<bool> {
    use std::fs::OpenOptions;
    use std::io::Write;

    if verbose {
        println!("    → Finalizing gcloud setup on remote (single connection)...");
    }
    info!("Phase 3: Finalizing gcloud setup on {}", workspace);

    if let Some(log_path) = log_file {
        if let Some(proj) = project {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
                let _ = writeln!(file, "Setting gcloud project: {proj}");
            }
        }
    }

    let mut finalize_script = String::from(
        r#"set -e

# Move credentials from staging to final locations
ls -la ~/.secrets-sync/gcloud/ 2>&1 || { echo "ERROR: No secrets-sync/gcloud dir!"; exit 1; }

mkdir -p ~/.config/gcloud ~/.config/gcloud/configurations
rsync -a ~/.secrets-sync/gcloud/ ~/.config/gcloud/ 2>&1

if [ ! -f ~/.config/gcloud/application_default_credentials.json ]; then
    echo "ERROR: application_default_credentials.json not copied!"
    exit 1
fi

mkdir -p /tmp/.config/gcloud/configurations
rsync -a ~/.secrets-sync/gcloud/ /tmp/.config/gcloud/ 2>&1

# ~/.config/gcloud is volume-mounted into Docker containers (e.g. cloud-sql-proxy)
# which run as a different UID, so it needs to be world-readable
chmod 755 ~/.config/gcloud ~/.config/gcloud/configurations 2>/dev/null || true
chmod 644 ~/.config/gcloud/application_default_credentials.json 2>/dev/null || true
chmod 755 /tmp/.config/gcloud /tmp/.config/gcloud/configurations 2>/dev/null || true
chmod 644 /tmp/.config/gcloud/application_default_credentials.json 2>/dev/null || true

if gcloud auth application-default print-access-token >/dev/null 2>&1; then
    echo "DEVDOCK_ADC:valid"
else
    echo "DEVDOCK_ADC:invalid"
fi

# Authenticate Docker with GCP Artifact Registry using the synced gcloud credentials
if command -v docker >/dev/null 2>&1; then
    if gcloud auth print-access-token 2>/dev/null | docker login -u oauth2accesstoken --password-stdin https://us-central1-docker.pkg.dev >/dev/null 2>&1; then
        echo "DEVDOCK_DOCKER_LOGIN:success"
    else
        echo "DEVDOCK_DOCKER_LOGIN:failed"
    fi
else
    echo "DEVDOCK_DOCKER_LOGIN:skipped"
fi

rm -rf ~/.secrets-sync

# Verify gcloud auth (with timeout to avoid hangs)
AUTH_ACCOUNT=$(timeout 10 gcloud auth list --filter=status:ACTIVE --format='value(account)' 2>/dev/null || true)
if [ -n "$AUTH_ACCOUNT" ]; then
    echo "DEVDOCK_AUTH:$AUTH_ACCOUNT"
else
    echo "DEVDOCK_AUTH:none"
fi
"#,
    );

    if let Some(proj) = project {
        finalize_script.push_str(&format!(
            "\ngcloud config set project '{}' 2>&1 || echo 'DEVDOCK_WARN:project_set_failed'\n",
            proj
        ));
    }

    let output = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg(&finalize_script)
        .output()
        .map_err(|e| RemoteError::ConfigSyncError(format!("Phase 3 SSH failed: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if verbose {
        println!();
        println!("    === Credential Sync Debug ===");
        for line in stdout.lines() {
            if !line.starts_with("DEVDOCK_") {
                println!("    {line}");
            }
        }
        if !stderr.is_empty() {
            println!("    STDERR: {stderr}");
        }
        println!("    === End Debug ===");
        println!();
    }

    if !output.status.success() {
        return Err(RemoteError::ConfigSyncError(format!(
            "Gcloud finalization failed: {}\nstdout: {}\nstderr: {}",
            output.status, stdout, stderr
        )));
    }

    // Parse structured output markers
    let mut adc_expired = false;

    for line in stdout.lines() {
        if let Some(account) = line.strip_prefix("DEVDOCK_AUTH:") {
            if account != "none" {
                info!("gcloud credentials synced and verified: {}", account);
                if verbose {
                    println!("      (authenticated as: {})", account);
                }
            } else if verbose {
                println!("      (no active account found)");
            }
        }

        if line == "DEVDOCK_ADC:invalid" {
            adc_expired = true;
            if stderr.contains("invalid_grant")
                || stderr.contains("refreshing your current auth tokens")
            {
                eprintln!("\n⚠️  WARNING: Synced gcloud credentials appear to be expired!");
                eprintln!("   You will need to authenticate on the remote workspace:");
                eprintln!("     1. coder ssh {workspace}");
                eprintln!("     2. gcloud auth login");
                eprintln!("     3. gcloud auth application-default login");
                eprintln!();
            }
        }

        if let Some(status) = line.strip_prefix("DEVDOCK_DOCKER_LOGIN:") {
            match status {
                "success" => {
                    info!("Docker login to GCP Artifact Registry succeeded");
                    if verbose {
                        println!("      (docker login to us-central1-docker.pkg.dev: ok)");
                    }
                }
                "failed" => {
                    debug!("Docker login to GCP Artifact Registry failed");
                    if verbose {
                        println!("      (docker login to us-central1-docker.pkg.dev: failed — images may fail to pull)");
                    }
                }
                "skipped" => {
                    debug!("Docker not available on remote, skipping docker login");
                }
                _ => {}
            }
        }

        if line.contains("DEVDOCK_WARN:project_set_failed") {
            debug!("Failed to set gcloud project");
            if let Some(log_path) = log_file {
                if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
                    let _ = writeln!(file, "WARNING: Failed to set gcloud project");
                }
            }
        }
    }

    if let Some(log_path) = log_file {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
            let _ = writeln!(file, "✓ gcloud setup finalized");
        }
    }

    debug!("Phase 3 complete: gcloud setup finalized");
    Ok(adc_expired)
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
        assert!(script.contains("~/.secrets-sync/git-creds/gh/hosts.yml"));
    }
}
