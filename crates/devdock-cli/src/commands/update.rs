//! Update command implementation for self-updating devdock binary.

use crate::output;
use anyhow::{anyhow, Context, Result};
use dialoguer::Confirm;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const REPO: &str = "askscio/devdock";
const BINARY_NAME: &str = "devdock";

/// Version config stored in ~/.config/devdock/version.yaml
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct VersionConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pinned: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_check: Option<String>,
    #[serde(default = "default_auto_check")]
    auto_check: bool,
}

fn default_auto_check() -> bool {
    true
}

/// Parse version from input string (handles optional @ prefix)
/// Examples:
///   None -> None (latest)
///   "0.2.7" -> Some("0.2.7")
///   "@0.2.7" -> Some("0.2.7")
///   "latest" -> Some("latest")
fn parse_version_input(input: Option<&str>) -> Option<String> {
    input.map(|s| {
        // Remove @ prefix if present
        s.trim_start_matches('@').to_string()
    })
}

/// Get the config directory path
fn get_config_dir() -> Result<PathBuf> {
    let home = env::var("HOME").context("HOME environment variable not set")?;
    Ok(PathBuf::from(home).join(".config").join("devdock"))
}

/// Load version config
fn load_version_config() -> Result<VersionConfig> {
    let config_path = get_config_dir()?.join("version.yaml");

    if !config_path.exists() {
        return Ok(VersionConfig::default());
    }

    let content = fs::read_to_string(&config_path).context("Failed to read version config")?;

    serde_yaml::from_str(&content).context("Failed to parse version config")
}

/// Save version config
fn save_version_config(config: &VersionConfig) -> Result<()> {
    let config_dir = get_config_dir()?;
    fs::create_dir_all(&config_dir)?;

    let config_path = config_dir.join("version.yaml");
    let content = serde_yaml::to_string(config)?;

    fs::write(&config_path, content).context("Failed to save version config")
}

/// Get current installed version
fn get_current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Check if gh CLI is available
fn check_gh_cli() -> Result<()> {
    Command::new("gh")
        .arg("--version")
        .output()
        .context("GitHub CLI (gh) is not installed. Install it from: https://cli.github.com/")?;

    // Check authentication
    let auth_status = Command::new("gh").args(["auth", "status"]).output();

    if auth_status.is_err() || !auth_status.unwrap().status.success() {
        return Err(anyhow!("GitHub CLI is not authenticated. Run: gh auth login"));
    }

    Ok(())
}

/// Get latest release version from GitHub
fn get_latest_version() -> Result<String> {
    let output = Command::new("gh")
        .args([
            "release",
            "list",
            "--repo",
            REPO,
            "--limit",
            "1",
            "--json",
            "tagName",
            "--jq",
            ".[0].tagName",
        ])
        .output()
        .context("Failed to fetch latest version")?;

    if !output.status.success() {
        return Err(anyhow!("Failed to fetch latest version from GitHub"));
    }

    let version =
        String::from_utf8_lossy(&output.stdout).trim().trim_start_matches('v').to_string();

    if version.is_empty() {
        return Err(anyhow!("No releases found"));
    }

    Ok(version)
}

/// Get all available versions from GitHub
fn get_available_versions() -> Result<Vec<String>> {
    let output = Command::new("gh")
        .args([
            "release",
            "list",
            "--repo",
            REPO,
            "--limit",
            "20",
            "--json",
            "tagName",
            "--jq",
            ".[].tagName",
        ])
        .output()
        .context("Failed to fetch versions")?;

    if !output.status.success() {
        return Err(anyhow!("Failed to fetch versions from GitHub"));
    }

    let versions: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.trim().trim_start_matches('v').to_string())
        .filter(|s| !s.is_empty())
        .collect();

    Ok(versions)
}

/// Detect OS and architecture
fn detect_platform() -> Result<String> {
    let os = match env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        _ => return Err(anyhow!("Unsupported OS: {}", env::consts::OS)),
    };

    let arch = match env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "arm64",
        _ => return Err(anyhow!("Unsupported architecture: {}", env::consts::ARCH)),
    };

    Ok(format!("{}-{}", os, arch))
}

/// Download and install a specific version
fn download_and_install(version: &str) -> Result<PathBuf> {
    let platform = detect_platform()?;
    let tag = format!("v{}", version);
    let asset_name = format!("{}-{}-{}.tar.gz", BINARY_NAME, platform, tag);

    output::info(&format!("Downloading {} for {}...", tag, platform));

    // Create temp directory
    let temp_dir = tempfile::tempdir()?;
    let temp_path = temp_dir.path();

    // Download using gh CLI
    let download = Command::new("gh")
        .args([
            "release",
            "download",
            &tag,
            "--repo",
            REPO,
            "--pattern",
            &asset_name,
            "--dir",
            temp_path.to_str().unwrap(),
        ])
        .output()?;

    if !download.status.success() {
        let stderr = String::from_utf8_lossy(&download.stderr);
        return Err(anyhow!("Failed to download release: {}", stderr));
    }

    // Extract tarball
    output::info("Extracting archive...");
    let archive_path = temp_path.join(&asset_name);

    let extract = Command::new("tar")
        .args(["-xzf", archive_path.to_str().unwrap(), "-C", temp_path.to_str().unwrap()])
        .output()?;

    if !extract.status.success() {
        return Err(anyhow!("Failed to extract archive"));
    }

    let binary_path = temp_path.join(BINARY_NAME);

    if !binary_path.exists() {
        return Err(anyhow!("Binary not found in archive"));
    }

    // Get current binary location
    let current_exe = env::current_exe()?;

    output::info(&format!("Installing to {}...", current_exe.display()));

    // Check if we need sudo
    let parent_dir = current_exe.parent().unwrap();
    let needs_sudo = !parent_dir.metadata()?.permissions().readonly()
        && parent_dir.to_str().unwrap().starts_with("/usr");

    // Install the binary
    if needs_sudo {
        output::warn("Installation requires sudo access");

        let install = Command::new("sudo")
            .args(["mv", binary_path.to_str().unwrap(), current_exe.to_str().unwrap()])
            .status()?;

        if !install.success() {
            return Err(anyhow!("Failed to install binary (sudo required)"));
        }

        // Set permissions
        Command::new("sudo").args(["chmod", "+x", current_exe.to_str().unwrap()]).status()?;
    } else {
        fs::copy(&binary_path, &current_exe)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&current_exe)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&current_exe, perms)?;
        }
    }

    Ok(current_exe)
}

/// Execute the update command
pub async fn execute(
    version_input: Option<String>,
    check_only: bool,
    force: bool,
    list: bool,
) -> Result<()> {
    // List versions and exit
    if list {
        return list_versions().await;
    }

    // Check only and exit
    if check_only {
        return check_for_updates().await;
    }

    // Check gh CLI
    check_gh_cli()?;

    let current_version = get_current_version();
    let config = load_version_config()?;

    output::header("DevDock Update");
    output::info(&format!("Current version: {}", current_version));

    // Parse requested version
    let requested_version = parse_version_input(version_input.as_deref());

    // Determine target version
    let target_version = if let Some(ver) = requested_version.as_ref() {
        if ver == "latest" {
            get_latest_version()?
        } else {
            ver.clone()
        }
    } else {
        get_latest_version()?
    };

    // Check if pinned and not forcing
    if let Some(pinned) = config.pinned.as_ref() {
        if !force && requested_version.is_none() {
            // Check if pin is stale (pinned version != current version)
            if pinned != &current_version {
                output::warn(&format!(
                    "Stale pin detected: pinned to {} but currently on {}",
                    pinned, current_version
                ));
                output::info("Clearing stale pin...");

                // Clear the stale pin
                let mut new_config = config.clone();
                new_config.pinned = None;
                save_version_config(&new_config)?;

                output::info("Pin cleared. Checking for updates...");
                // Continue with normal update flow
            } else {
                // Pin matches current version
                output::info(&format!("Pinned to version {}", pinned));

                let latest = get_latest_version()?;
                if latest != *pinned {
                    output::info(&format!("Latest version: {} available", latest));
                    output::info("Run 'devdock update --force' to update and remove pin");
                } else {
                    output::success("Already on latest version (pinned)");
                }

                return Ok(());
            }
        }
    }

    // Check if already on target version
    if current_version == target_version && !force {
        output::success(&format!("Already on version {}", target_version));
        return Ok(());
    }

    output::info(&format!("Target version: {}", target_version));

    // Confirm if not forcing
    if !force {
        let confirmed = Confirm::new()
            .with_prompt(format!("Update to version {}?", target_version))
            .default(true)
            .interact()?;

        if !confirmed {
            output::info("Update cancelled");
            return Ok(());
        }
    }

    // Download and install
    download_and_install(&target_version)?;

    // Update config
    let mut new_config = config;
    new_config.last_check = Some(chrono::Utc::now().to_rfc3339());

    // Pin if specific version requested, unpin if "latest" or no version specified
    if let Some(ver) = requested_version.as_ref() {
        if ver != "latest" {
            new_config.pinned = Some(target_version.clone());
            output::info(&format!("Pinned to version {}", target_version));
        } else {
            new_config.pinned = None;
        }
    } else {
        new_config.pinned = None;
    }

    save_version_config(&new_config)?;

    output::success(&format!("✅ Updated to version {}", target_version));
    output::info("Restart any running devdock instances to use the new version");

    Ok(())
}

/// List available versions
async fn list_versions() -> Result<()> {
    output::header("Available Versions");

    check_gh_cli()?;

    let current = get_current_version();
    let versions = get_available_versions()?;
    let config = load_version_config()?;

    output::info(&format!("Current version: {}", current));

    if let Some(pinned) = config.pinned {
        output::info(&format!("Pinned to: {}", pinned));
    }

    println!();
    output::info("Available versions:");

    for (i, version) in versions.iter().enumerate() {
        let mut line = format!("  {}", version);

        if version == &current {
            line.push_str(" (current)");
        }

        if i == 0 {
            line.push_str(" (latest)");
        }

        println!("{}", line);
    }

    Ok(())
}

/// Check for updates without installing
async fn check_for_updates() -> Result<()> {
    check_gh_cli()?;

    let current = get_current_version();
    let latest = get_latest_version()?;
    let config = load_version_config()?;

    output::info(&format!("Current version: {}", current));
    output::info(&format!("Latest version: {}", latest));

    if let Some(pinned) = config.pinned {
        output::info(&format!("Pinned to: {}", pinned));
    }

    if current == latest {
        output::success("You are on the latest version");
    } else {
        output::warn(&format!("Update available: {} -> {}", current, latest));
        output::info("Run 'devdock update' to upgrade");
    }

    Ok(())
}

/// The dev version suffix set at build time (non-empty for local/debug builds).
const DEV_VERSION_SUFFIX: &str = env!("DEV_VERSION_SUFFIX");

/// Check for updates on startup (called from main)
pub async fn check_on_startup() -> Result<()> {
    // Dev builds (local cargo build) should never auto-update
    #[allow(clippy::const_is_empty)]
    if !DEV_VERSION_SUFFIX.is_empty() {
        let full_version = format!("{}{}", get_current_version(), DEV_VERSION_SUFFIX);
        output::info(&format!("Running dev build ({}) — skipping auto-update check", full_version));
        return Ok(());
    }

    let config = load_version_config().unwrap_or_default();

    // Skip if auto-check is disabled
    if !config.auto_check {
        return Ok(());
    }

    // Skip if checked recently (within last 24 hours)
    if let Some(last_check) = config.last_check.as_ref() {
        if let Ok(last_time) = chrono::DateTime::parse_from_rfc3339(last_check) {
            let now = chrono::Utc::now();
            let duration = now.signed_duration_since(last_time);

            if duration.num_hours() < 24 {
                return Ok(());
            }
        }
    }

    // Try to check for updates (silently fail if offline)
    if check_gh_cli().is_err() {
        return Ok(());
    }

    let current = get_current_version();

    if let Ok(latest) = get_latest_version() {
        if current != latest {
            if let Some(ref pinned) = config.pinned {
                output::info(&format!(
                    "ℹ You're on pinned version {} (latest: {})",
                    pinned, latest
                ));
                output::info("  Run 'devdock update --force' to update");
            } else {
                output::warn(&format!(
                    "⚠ New version {} available! Run 'devdock update' to upgrade",
                    latest
                ));
            }
            println!();
        }

        // Update last check time
        let mut new_config = config;
        new_config.last_check = Some(chrono::Utc::now().to_rfc3339());
        let _ = save_version_config(&new_config);
    }

    Ok(())
}
