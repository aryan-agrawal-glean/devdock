//! Docker utilities for DevDock.
//!
//! Provides functions for:
//! - Checking if Docker is running
//! - Setting up Docker networks
//! - Cleaning up containers and images
//! - Managing Docker images

use anyhow::{Context, Result};
use std::process::Command;
use tracing::{info, warn};

/// Required Docker networks for DevDock
pub const DEVDOCK_NETWORKS: &[&str] = &["devdock_network", "devdock_infra"];

/// Docker images required for DevDock
pub const REQUIRED_IMAGES: &[&str] = &[
    "us-central1-docker.pkg.dev/scio-engineering/glean-images/devdock_python_base:latest",
    "memcached:alpine3.21",
    "debian:bullseye-slim",
    "mcr.microsoft.com/azure-messaging/servicebus-emulator:latest",
    "nginx:1.27.3-alpine",
    "gcr.io/google.com/cloudsdktool/google-cloud-cli:emulators",
    "mysql:8.0.37",
    "gcr.io/cloud-sql-connectors/cloud-sql-proxy:2.14.3",
    "maven:3.9.6-eclipse-temurin-17",
    "eclipse-temurin:17-jdk",
    "redis:7.2-alpine",
    "grafana/loki:latest",
    "grafana/alloy:latest",
    "minio/minio",
    "grafana/grafana:latest",
];

/// Python-only images (subset for faster updates)
pub const PYTHON_IMAGES: &[&str] =
    &["us-central1-docker.pkg.dev/scio-engineering/glean-images/devdock_python_base:latest"];

/// Rich logging images (Grafana stack)
pub const RICH_LOGGING_IMAGES: &[&str] = &[
    "grafana/loki:latest",
    "grafana/alloy:latest",
    "minio/minio",
    "grafana/grafana:latest",
    "nginx:latest",
    "prom/prometheus:latest",
];

/// Check if Docker daemon is running
pub fn check_docker() -> Result<()> {
    info!("Checking if Docker is running...");

    let output = Command::new("docker")
        .args(["info"])
        .output()
        .context("Failed to execute docker info command")?;

    if !output.status.success() {
        anyhow::bail!(
            "Docker is not running.\n\n\
            Please start the Docker engine and try again.\n\
            To start Docker with Colima, run:\n\
            \n\
            colima delete --force -p default -p slirp\n\
            colima start --memory 10 --cpu 4 --network-address slirp --vm-type vz"
        );
    }

    info!("Docker is running");
    Ok(())
}

/// Setup required Docker networks
pub fn setup_networks() -> Result<()> {
    info!("Setting up Docker networks...");

    for network in DEVDOCK_NETWORKS {
        // Check if network exists
        let check = Command::new("docker").args(["network", "inspect", network]).output()?;

        if !check.status.success() {
            info!("Creating Docker network: {}", network);
            let create = Command::new("docker")
                .args(["network", "create", network])
                .output()
                .context(format!("Failed to create network {network}"))?;

            if !create.status.success() {
                warn!(
                    "Failed to create network {}: {}",
                    network,
                    String::from_utf8_lossy(&create.stderr)
                );
            }
        }
    }

    info!("Docker networks are ready");
    Ok(())
}

/// Get all devdock container names
pub fn get_devdock_containers() -> Result<Vec<String>> {
    let output = Command::new("docker")
        .args(["ps", "-a", "--filter", "name=devdock-", "--format", "{{.Names}}"])
        .output()
        .context("Failed to list containers")?;

    if output.status.success() {
        let names: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|s| !s.is_empty())
            .map(std::string::ToString::to_string)
            .collect();
        Ok(names)
    } else {
        Ok(vec![])
    }
}

/// Stop and remove a container
pub fn remove_container(name: &str) -> Result<()> {
    // Stop container (ignore errors if not running)
    let _ = Command::new("docker").args(["stop", name]).output();

    // Remove container
    let _ = Command::new("docker").args(["rm", "-f", name]).output();

    Ok(())
}

/// Remove dangling images
pub fn remove_dangling_images() -> Result<usize> {
    let output = Command::new("docker")
        .args(["images", "--filter", "dangling=true", "-q"])
        .output()
        .context("Failed to list dangling images")?;

    if !output.status.success() {
        return Ok(0);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let image_ids: Vec<&str> = stdout.lines().filter(|s| !s.is_empty()).collect();

    let count = image_ids.len();

    if !image_ids.is_empty() {
        for id in &image_ids {
            let _ = Command::new("docker").args(["rmi", id]).output();
        }
    }

    Ok(count)
}

/// Execute a command in a container
pub fn exec_container(container: &str, command: &str) -> Result<()> {
    // Map common aliases
    let actual_container = match container {
        "sql" => "devdock-mysql",
        _ => &format!("devdock-{container}"),
    };

    // Determine shell/command based on container
    let shell = if container == "sql" || container == "mysql" { "mysql" } else { "sh" };

    let cmd = if command.is_empty() { shell } else { command };

    // Execute interactively
    let status = Command::new("docker")
        .args(["exec", "-it", actual_container, cmd])
        .status()
        .context(format!("Failed to exec into container {actual_container}"))?;

    if !status.success() {
        anyhow::bail!(
            "Failed to exec into container. Either the container doesn't exist or there's a typo in the command."
        );
    }

    Ok(())
}

/// Pull Docker images
pub fn pull_images(images: &[&str], save_cache: bool) -> Result<()> {
    let home = std::env::var("HOME").unwrap_or_default();
    let cache_dir = format!("{home}/.devdock/images");

    if save_cache {
        std::fs::create_dir_all(&cache_dir)?;
    }

    for image in images {
        info!("Pulling Docker image: {}", image);

        let output = Command::new("docker")
            .args(["pull", image])
            .output()
            .context(format!("Failed to pull image {image}"))?;

        if !output.status.success() {
            warn!("Failed to pull {}: {}", image, String::from_utf8_lossy(&output.stderr));
            continue;
        }

        // Save to cache if requested
        if save_cache {
            let image_name = extract_image_name(image);
            let cache_file = format!("{cache_dir}/{image_name}.zst");

            info!("Saving {} to cache...", image_name);

            // docker save | zstd > cache_file
            let save = Command::new("sh")
                .args(["-c", &format!("docker save '{image}' | zstd -f -o '{cache_file}'")])
                .output();

            if let Err(e) = save {
                warn!("Failed to cache {}: {}", image, e);
            }
        }
    }

    Ok(())
}

/// Load image from cache
pub fn load_cached_image(image: &str) -> Result<bool> {
    let home = std::env::var("HOME").unwrap_or_default();
    let image_name = extract_image_name(image);
    let cache_file = format!("{home}/.devdock/images/{image_name}.zst");

    if !std::path::Path::new(&cache_file).exists() {
        return Ok(false);
    }

    info!("Loading cached image: {}", image_name);

    let output = Command::new("sh")
        .args(["-c", &format!("zstd -dc '{cache_file}' | docker load")])
        .output()
        .context(format!("Failed to load cached image {image_name}"))?;

    Ok(output.status.success())
}

/// Extract image name from full image path
/// e.g., "us-central1-docker.pkg.dev/scio-engineering/glean-images/devdock_python_base:latest"
///       -> "devdock_python_base"
fn extract_image_name(image: &str) -> String {
    // Split by '/' and get the last part, then split by ':' and get the first part
    image.rsplit('/').next().unwrap_or(image).split(':').next().unwrap_or(image).to_string()
}

/// Cleanup ports that might be in use
pub fn cleanup_ports(ports: &[u16]) -> Result<usize> {
    let mut killed = 0;

    for port in ports {
        let output = Command::new("lsof").args(["-i", &format!(":{port}")]).output();

        if let Ok(out) = output {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                for line in stdout.lines().skip(1) {
                    // Second column is PID
                    if let Some(pid_str) = line.split_whitespace().nth(1) {
                        if let Ok(pid) = pid_str.parse::<i32>() {
                            info!("Killing process {} on port {}", pid, port);
                            let _ = Command::new("kill").args(["-15", &pid.to_string()]).output();
                            killed += 1;

                            // Wait a bit then force kill if still running
                            std::thread::sleep(std::time::Duration::from_secs(1));
                            let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
                        }
                    }
                }
            }
        }
    }

    Ok(killed)
}

/// Authenticate with GCR/Artifact Registry
pub fn docker_login_gcr() -> Result<()> {
    info!("Authenticating with Google Container Registry...");

    // Get access token
    let token_output = Command::new("gcloud")
        .args(["auth", "print-access-token"])
        .output()
        .context("Failed to get gcloud access token")?;

    if !token_output.status.success() {
        anyhow::bail!("Failed to get gcloud access token");
    }

    let token = String::from_utf8_lossy(&token_output.stdout).trim().to_string();

    // Login to artifact registry. This must wait for docker login to finish; otherwise
    // callers can proceed as if auth succeeded while Docker is still failing in the
    // background.
    let mut login = Command::new("docker")
        .args([
            "login",
            "-u",
            "oauth2accesstoken",
            "--password-stdin",
            "https://us-central1-docker.pkg.dev",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Failed to start docker login")?;

    if let Some(mut stdin) = login.stdin.take() {
        use std::io::Write;
        stdin.write_all(token.as_bytes()).context("Failed to write token to docker login")?;
    }

    let output = login.wait_with_output().context("Failed to wait for docker login")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        anyhow::bail!(
            "Docker login to us-central1-docker.pkg.dev failed.\nstdout: {}\nstderr: {}",
            stdout.trim(),
            stderr.trim()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_image_name() {
        assert_eq!(
            extract_image_name("us-central1-docker.pkg.dev/scio-engineering/glean-images/devdock_python_base:latest"),
            "devdock_python_base"
        );
        assert_eq!(extract_image_name("grafana/grafana:latest"), "grafana");
        assert_eq!(extract_image_name("nginx:1.27.3-alpine"), "nginx");
        assert_eq!(extract_image_name("redis"), "redis");
    }
}
