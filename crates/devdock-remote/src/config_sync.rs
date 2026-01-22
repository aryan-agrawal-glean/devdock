//! Configuration file synchronization to remote workspaces.

use crate::{RemoteError, Result};
use devdock_core::profile::Profile;
use std::path::Path;
use std::process::Command;
use tracing::{debug, info};

/// Sync profile YAML to remote workspace.
pub fn sync_profile(workspace: &str, profile_path: &Path, verbose: bool) -> Result<()> {
    info!("Syncing profile to {}: {:?}", workspace, profile_path);
    if verbose {
        println!("    → Syncing profile...");
    }

    if !profile_path.exists() {
        return Err(RemoteError::ConfigSyncError(format!("Profile not found: {profile_path:?}")));
    }

    // Note: Directory creation is handled by sync_all_config for efficiency
    debug!("Copying profile using coder: {:?} -> {}", profile_path, workspace);

    // Read the file content
    let content = std::fs::read_to_string(profile_path)
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to read profile: {e}")))?;

    // Write it to remote using cat via SSH
    let filename = profile_path
        .file_name()
        .ok_or_else(|| RemoteError::ConfigSyncError("Invalid profile path".to_string()))?
        .to_string_lossy();

    let write_cmd = format!(
        "cat > /home/workspace/scio/devdock/profiles/{filename} << 'DEVDOCK_EOF'\n{content}\nDEVDOCK_EOF"
    );

    let output = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg("bash")
        .arg("-c")
        .arg(&write_cmd)
        .output()
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to copy profile: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RemoteError::ConfigSyncError(stderr.to_string()));
    }

    debug!("Profile synced successfully");
    Ok(())
}

/// Sync custom.ini to remote workspace if it exists.
pub fn sync_custom_ini(
    workspace: &str,
    custom_ini_path: Option<&Path>,
    verbose: bool,
) -> Result<()> {
    if let Some(path) = custom_ini_path {
        if path.exists() {
            info!("Syncing custom.ini to {}", workspace);
            if verbose {
                println!("    → Syncing custom.ini...");
            }

            // Note: Directory creation is handled by sync_all_config for efficiency

            // Read and write file via SSH
            let content = std::fs::read_to_string(path).map_err(|e| {
                RemoteError::ConfigSyncError(format!("Failed to read custom.ini: {e}"))
            })?;

            let write_cmd = format!(
                "cat > /home/workspace/scio/config/custom.ini << 'DEVDOCK_EOF'\n{content}\nDEVDOCK_EOF"
            );

            let output = Command::new("coder")
                .arg("ssh")
                .arg(workspace)
                .arg("--")
                .arg("bash")
                .arg("-c")
                .arg(&write_cmd)
                .output()
                .map_err(|e| {
                    RemoteError::ConfigSyncError(format!("Failed to copy custom.ini: {e}"))
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(RemoteError::ConfigSyncError(stderr.to_string()));
            }

            debug!("custom.ini synced successfully");
        }
    }
    Ok(())
}

/// Sync gcloud credentials to remote workspace.
pub fn sync_gcloud_credentials(workspace: &str, verbose: bool) -> Result<()> {
    info!("Syncing gcloud credentials to {}", workspace);
    if verbose {
        println!("    → Syncing gcloud credentials (this may take 10-20s)...");
    }

    // Check if gcloud credentials exist locally
    let gcloud_dir = dirs::home_dir()
        .ok_or_else(|| RemoteError::ConfigSyncError("Cannot determine home directory".to_string()))?
        .join(".config/gcloud");

    if !gcloud_dir.exists() {
        debug!("No ~/.config/gcloud found; skipping gcloud sync");
        if verbose {
            println!("    → No local gcloud credentials found, skipping");
        }
        return Ok(());
    }

    // Create temp directory for staging
    let temp_dir = tempfile::tempdir().map_err(|e| {
        RemoteError::ConfigSyncError(format!("Failed to create temp directory: {e}"))
    })?;
    let temp_path = temp_dir.path();
    let gcloud_temp = temp_path.join("gcloud");

    std::fs::create_dir_all(&gcloud_temp).map_err(|e| {
        RemoteError::ConfigSyncError(format!("Failed to create gcloud temp dir: {e}"))
    })?;
    std::fs::create_dir_all(gcloud_temp.join("configurations")).map_err(|e| {
        RemoteError::ConfigSyncError(format!("Failed to create configurations dir: {e}"))
    })?;

    // Core files needed for gcloud auth
    let gcloud_files = vec![
        "application_default_credentials.json",
        "access_tokens.db",
        "credentials.db",
        "active_config",
        "configurations/config_default",
        "env.yaml",
    ];

    // Copy files to temp directory
    info!("Staging gcloud credential files...");
    let mut staged_files = Vec::new();

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

            // Set permissions to 0600
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
            staged_files.push(file);
        }
    }

    // Verify we have the critical ADC file
    if !gcloud_temp.join("application_default_credentials.json").exists() {
        return Err(RemoteError::ConfigSyncError(
            "application_default_credentials.json not found locally - run: gcloud auth application-default login".to_string()
        ));
    }

    if verbose {
        println!("    → Staged {} credential files", staged_files.len());
    }
    info!("Staged {} gcloud files", staged_files.len());

    // Note: Remote directory ~/.secrets-sync is created by sync_all_config for efficiency

    // Copy to workspace using scp
    info!("Copying gcloud credentials to remote...");
    if verbose {
        println!("      (copying files via scp...)");
    }
    let gcloud_temp_str = gcloud_temp.to_string_lossy();
    let scp_target = format!("{workspace}.coder:~/.secrets-sync/");

    let output = Command::new("scp")
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

    // Move to final location on remote with verification
    debug!("Moving credentials to final locations...");
    let move_script = r#"
        set -e

        # Debug: Check what was actually synced
        echo "=== Files synced to staging ==="
        ls -la ~/.secrets-sync/gcloud/ 2>&1 || echo "ERROR: No secrets-sync/gcloud dir!"

        # Copy to home directory for gcloud CLI (DON'T suppress errors)
        echo "=== Copying to ~/.config/gcloud ==="
        mkdir -p ~/.config/gcloud ~/.config/gcloud/configurations
        rsync -av ~/.secrets-sync/gcloud/ ~/.config/gcloud/ 2>&1

        # Verify the critical ADC file
        if [ ! -f ~/.config/gcloud/application_default_credentials.json ]; then
            echo "ERROR: application_default_credentials.json not copied!"
            exit 1
        fi

        # Also copy to /tmp/.config for services
        echo "=== Copying to /tmp/.config/gcloud ==="
        mkdir -p /tmp/.config/gcloud/configurations
        rsync -av ~/.secrets-sync/gcloud/ /tmp/.config/gcloud/ 2>&1

        # Set proper permissions
        chmod 700 ~/.config/gcloud ~/.config/gcloud/configurations 2>/dev/null || true
        chmod 700 /tmp/.config/gcloud /tmp/.config/gcloud/configurations 2>/dev/null || true
        chmod 600 ~/.config/gcloud/application_default_credentials.json 2>/dev/null || true
        chmod 600 /tmp/.config/gcloud/application_default_credentials.json 2>/dev/null || true

        # Verify ADC works after copy
        echo "=== Testing ADC on remote ==="
        if gcloud auth application-default print-access-token >/dev/null 2>&1; then
            echo "✓ ADC is valid on remote"
        else
            echo "⚠ ADC test failed (will check again after project set)"
        fi

        # Clean up staging directory
        rm -rf ~/.secrets-sync

        echo "=== Credential sync complete ==="
    "#;

    let output = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg(move_script)
        .output()
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to move credentials: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Always show debug output in verbose mode
    if verbose {
        println!();
        println!("    === Credential Sync Debug ===");
        for line in stdout.lines() {
            println!("    {line}");
        }
        if !stderr.is_empty() {
            println!("    STDERR: {stderr}");
        }
        println!("    === End Debug ===");
        println!();
    }

    if !output.status.success() {
        return Err(RemoteError::ConfigSyncError(format!(
            "Credential sync failed: {}\nstdout: {}\nstderr: {}",
            output.status, stdout, stderr
        )));
    }

    // Verify gcloud auth actually works (not just config exists)
    debug!("Verifying gcloud authentication...");
    if verbose {
        println!("    → Verifying gcloud authentication...");
    }

    // Use Rust's spawn + timeout, pass command as single string (not via bash -c)
    let mut child = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg("gcloud auth list --filter=status:ACTIVE --format=value(account)")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to spawn gcloud verify: {e}")))?;

    // Wait with 10 second timeout
    use std::time::{Duration, Instant};
    let start = Instant::now();
    let timeout = Duration::from_secs(10);

    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Process completed
                let output = child.wait_with_output().map_err(|e| {
                    RemoteError::ConfigSyncError(format!("Failed to read output: {e}"))
                })?;
                break Some((status, output));
            }
            Ok(None) => {
                // Still running
                if start.elapsed() > timeout {
                    // Timeout - kill it
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => {
                return Err(RemoteError::ConfigSyncError(format!("Failed to check status: {e}")));
            }
        }
    };

    if let Some((status, output)) = result {
        let auth_output = String::from_utf8_lossy(&output.stdout);
        let auth_error = String::from_utf8_lossy(&output.stderr);

        if status.success() && !auth_output.trim().is_empty() {
            info!("gcloud credentials synced and verified: {}", auth_output.trim());
            if verbose {
                println!("      (authenticated as: {})", auth_output.trim());
            }
        } else {
            // Credentials may be stale - warn but don't fail
            if auth_error.contains("invalid_grant")
                || auth_error.contains("refreshing your current auth tokens")
            {
                eprintln!("\n⚠️  WARNING: Synced gcloud credentials appear to be expired!");
                eprintln!("   You will need to authenticate on the remote workspace:");
                eprintln!("     1. coder ssh {workspace}");
                eprintln!("     2. gcloud auth login");
                eprintln!("     3. gcloud auth application-default login");
                eprintln!();
            } else if !auth_output.trim().is_empty() || !auth_error.trim().is_empty() {
                debug!("gcloud auth check completed but no active account: {}", auth_error);
                if verbose {
                    println!("      (no active account found)");
                }
            }
        }
    } else {
        eprintln!("\n⚠️  WARNING: gcloud auth verification timed out after 10s!");
        eprintln!("   This may indicate network issues or gcloud is hung.");
        eprintln!("   Continuing anyway, but DevDock startup may fail.");
        eprintln!();
    }

    Ok(())
}

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

    // Check required AWS environment variables
    let has_access_key = std::env::var("AWS_ACCESS_KEY_ID").is_ok();
    let has_secret_key = std::env::var("AWS_SECRET_ACCESS_KEY").is_ok();
    let has_region =
        std::env::var("AWS_REGION").is_ok() || std::env::var("AWS_DEFAULT_REGION").is_ok();

    // Fail fast if required credentials are missing
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

    // Warn if region is missing
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

/// Set gcloud project on remote workspace.
pub fn set_gcloud_project(workspace: &str, project: &str, verbose: bool) -> Result<()> {
    info!("Setting gcloud project to: {}", project);
    if verbose {
        println!("    → Setting gcloud project: {project}...");
    }

    let output = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg("gcloud")
        .arg("config")
        .arg("set")
        .arg("project")
        .arg(project)
        .output()
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to set gcloud project: {e}")))?;

    if output.status.success() {
        debug!("gcloud project set successfully");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!("Warning: Failed to set gcloud project: {}", stderr);
        // Don't fail if gcloud config set fails - it may not be critical
    }

    Ok(())
}

/// Sync all configuration files to remote workspace.
pub fn sync_all_config(
    workspace: &str,
    profile_path: &Path,
    custom_ini_path: Option<&Path>,
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

    // Optimization: Create all needed directories in a single SSH call
    // This saves ~2 seconds compared to separate mkdir calls
    if verbose {
        println!("    → Creating directories on remote...");
    }

    // Create directories - clean .secrets-sync first to ensure it's not a stale file
    // Note: coder ssh already invokes a shell, so we don't need bash -c
    let mkdir_output = Command::new("coder")
        .arg("ssh")
        .arg(workspace)
        .arg("--")
        .arg("rm -rf ~/.secrets-sync && mkdir -p /home/workspace/scio/devdock/profiles/ /home/workspace/scio/config/ ~/.secrets-sync")
        .output()
        .map_err(|e| RemoteError::ConfigSyncError(format!("Failed to create remote directories: {e}")))?;

    if !mkdir_output.status.success() {
        let stderr = String::from_utf8_lossy(&mkdir_output.stderr);
        let stdout = String::from_utf8_lossy(&mkdir_output.stdout);
        return Err(RemoteError::ConfigSyncError(format!(
            "Failed to create directories on remote: {}\nstdout: {}\nstderr: {}",
            mkdir_output.status, stdout, stderr
        )));
    }

    sync_profile(workspace, profile_path, verbose)?;
    sync_custom_ini(workspace, custom_ini_path, verbose)?;

    // Get platform from profile to determine which credentials to sync
    let platform = get_profile_platform(profile_path).ok();

    // Conditionally sync credentials based on platform
    match platform.as_deref() {
        Some("google") | Some("gcp") => {
            // Only sync gcloud credentials for Google Cloud profiles
            if let Err(e) = sync_gcloud_credentials(workspace, verbose) {
                return Err(RemoteError::ConfigSyncError(format!(
                    "Failed to sync gcloud credentials (required for platform 'google'): {e}"
                )));
            }
        }
        Some("aws") => {
            // Only sync AWS credentials for AWS profiles
            if let Err(e) = sync_aws_credentials(workspace, verbose) {
                return Err(RemoteError::ConfigSyncError(format!(
                    "Failed to sync AWS credentials (required for platform 'aws'): {e}"
                )));
            }
        }
        Some("azure") => {
            // Azure credentials are typically managed differently (az cli login)
            // No sync needed for now
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
            // Could not determine platform - be safe and sync both as best-effort
            if verbose {
                println!("    → Could not determine platform, syncing available credentials...");
            }
            let _ = sync_gcloud_credentials(workspace, verbose);
            let _ = sync_aws_credentials(workspace, verbose);
        }
    }

    // Set gcloud project from profile (only for Google Cloud platform)
    if matches!(platform.as_deref(), Some("google") | Some("gcp")) {
        match get_profile_project(profile_path) {
            Ok(project) => {
                if let Some(log_path) = log_file {
                    if let Ok(mut file) =
                        OpenOptions::new().create(true).append(true).open(log_path)
                    {
                        let _ = writeln!(file, "Setting gcloud project: {project}");
                    }
                }
                if let Err(e) = set_gcloud_project(workspace, &project, verbose) {
                    debug!("Failed to set gcloud project: {}", e);
                    if let Some(log_path) = log_file {
                        if let Ok(mut file) =
                            OpenOptions::new().create(true).append(true).open(log_path)
                        {
                            let _ = writeln!(file, "WARNING: Failed to set gcloud project: {e}");
                        }
                    }
                    // Continue even if setting project fails
                } else if let Some(log_path) = log_file {
                    if let Ok(mut file) =
                        OpenOptions::new().create(true).append(true).open(log_path)
                    {
                        let _ = writeln!(file, "✓ gcloud project set successfully");
                    }
                }
            }
            Err(e) => {
                debug!("Failed to get project from profile: {}", e);
                if let Some(log_path) = log_file {
                    if let Ok(mut file) =
                        OpenOptions::new().create(true).append(true).open(log_path)
                    {
                        let _ = writeln!(file, "WARNING: Failed to get project from profile: {e}");
                    }
                }
                // Continue even if we can't get the project
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
