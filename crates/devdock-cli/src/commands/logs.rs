//! Rich logging management (Grafana/Loki/Alloy stack).

use crate::output;
use anyhow::{Context, Result};
use devdock_config::docker;
use std::path::PathBuf;
use std::process::Command;

/// Enable rich logging in quiet mode (for use with --enable-logs flag)
pub async fn enable_quiet() -> Result<()> {
    enable_internal(true).await
}

/// Find the observability config directory
fn find_observability_config_dir() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let home = std::env::var("HOME").unwrap_or_default();

    // Try locations in priority order
    let search_paths = vec![
        // 1. Development: Running from devdock repo root
        cwd.join("config/helpers/observability/grafana"),
        // 2. Common devdock repo location (primary)
        PathBuf::from(format!("{}/workspace/devdock/config/helpers/observability/grafana", home)),
        // 3. Alternative home location
        PathBuf::from(format!("{}/devdock/config/helpers/observability/grafana", home)),
        // 4. System-wide installation
        PathBuf::from("/usr/local/share/devdock/config/helpers/observability/grafana"),
    ];

    // Check each path
    for path in &search_paths {
        if path.exists() && path.join("docker-compose.yaml").exists() {
            return Ok(path.clone());
        }
    }

    // If running from scio monorepo, use those configs (optional fallback)
    if let Ok(root) = std::env::var("DEVDOCK_ROOT") {
        let monorepo_path =
            PathBuf::from(&root).join("devdock/config/helpers/observability/grafana");
        if monorepo_path.exists() && monorepo_path.join("docker-compose.yaml").exists() {
            return Ok(monorepo_path);
        }
    }

    anyhow::bail!(
        "Could not find observability config directory.\n\
        \n\
        The config files should be at one of:\n\
        - {}/workspace/devdock/config/helpers/observability/grafana\n\
        - Current directory: {}/config/helpers/observability/grafana\n\
        \n\
        These files are included in the devdock repository.\n\
        Please clone/update the devdock repo at $HOME/workspace/devdock/",
        home,
        cwd.display()
    )
}

/// Enable rich logging stack
pub async fn enable() -> Result<()> {
    enable_internal(false).await
}

/// Internal implementation with verbosity control
async fn enable_internal(quiet: bool) -> Result<()> {
    if !quiet {
        output::header("Enabling Rich Logging");
    }

    // Check Docker
    docker::check_docker()?;

    // Setup networks (use devdock_network as expected by the compose file)
    if !quiet {
        output::step(1, 4, "Setting up Docker networks...");
    }
    docker::setup_networks()?;
    if !quiet {
        output::success("Networks ready");
    }

    // Create directories
    if !quiet {
        output::step(2, 4, "Creating data directories...");
    }
    let home = std::env::var("HOME")?;
    let grafana_data_dir = PathBuf::from(&home).join(".devdock/grafana/.data/minio");
    std::fs::create_dir_all(&grafana_data_dir)?;

    // Set permissions with setgid bit (for Linux compatibility)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 2775 = setgid bit (2000) + rwxrwxr-x (0775)
        let grafana_dir = PathBuf::from(&home).join(".devdock/grafana");
        let perms = std::fs::Permissions::from_mode(0o2775);
        let _ = std::fs::set_permissions(&grafana_dir, perms);
    }
    if !quiet {
        output::success("Directories created");
    }

    // Load cached images or pull (in parallel for speed)
    if !quiet {
        output::step(3, 4, "Loading images...");
    }

    // Collect images that need to be pulled
    let mut images_to_pull = Vec::new();
    for image in docker::RICH_LOGGING_IMAGES {
        if !docker::load_cached_image(image)? {
            images_to_pull.push(*image);
        }
    }

    // Pull images in parallel if needed
    if !images_to_pull.is_empty() {
        use tokio::task;

        let pull_tasks: Vec<_> = images_to_pull
            .iter()
            .map(|&image| task::spawn_blocking(move || docker::pull_images(&[image], false)))
            .collect();

        // Wait for all pulls to complete
        for (i, task) in pull_tasks.into_iter().enumerate() {
            if let Err(e) = task.await? {
                if !quiet {
                    output::warn(&format!("Failed to pull {}: {}", images_to_pull[i], e));
                }
            }
        }
    }

    if !quiet {
        output::success("Images ready");
    }

    // Find the config directory
    if !quiet {
        output::step(4, 4, "Starting Grafana stack...");
    }
    let config_dir = find_observability_config_dir()?;
    let compose_path = config_dir.join("docker-compose.yaml");

    // Determine which docker compose command to use
    let docker_compose_cmd = if Command::new("docker")
        .args(["compose", "version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        vec!["docker", "compose"]
    } else if Command::new("docker-compose")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        vec!["docker-compose"]
    } else {
        anyhow::bail!("Neither 'docker compose' nor 'docker-compose' command found. Please install Docker Compose.");
    };

    // Run docker compose from the config directory (so relative paths in compose file work)
    let mut cmd = Command::new(docker_compose_cmd[0]);
    cmd.args(&docker_compose_cmd[1..])
        .args(["-f", compose_path.to_str().unwrap(), "up", "-d"])
        .current_dir(&config_dir); // Important: run from config dir for relative volume mounts

    // Suppress output in quiet mode
    if quiet {
        cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    }

    let status = cmd.status().context("Failed to start Grafana stack")?;

    if !status.success() {
        anyhow::bail!("Failed to start Grafana stack");
    }

    if !quiet {
        output::success(
            "Started Log viewer (Grafana). To view logs GOTO: http://localhost:3000/explore",
        );
    }

    Ok(())
}

/// Disable rich logging stack
pub async fn disable() -> Result<()> {
    output::header("Disabling Rich Logging");

    docker::check_docker()?;

    // Stop containers (from the production docker-compose)
    output::step(1, 2, "Stopping Grafana stack...");
    let containers = [
        "devdock-grafana-server",
        "devdock-grafana-loki-read",
        "devdock-grafana-loki-write",
        "devdock-grafana-loki-backend",
        "devdock-grafana-nginx",
        "devdock-grafana-alloy",
        "devdock-grafana-minio",
        "devdock-prometheus",
    ];

    for container in containers {
        let _ = docker::remove_container(container);
    }
    output::success("Containers stopped");

    // Clean up data
    output::step(2, 2, "Cleaning up data...");
    let home = std::env::var("HOME")?;
    let grafana_dir = PathBuf::from(&home).join(".devdock/grafana");
    if grafana_dir.exists() {
        let _ = std::fs::remove_dir_all(&grafana_dir);
    }
    output::success("Data cleaned");

    output::header("Rich Logging Disabled");
    output::success("✅ Rich logging has been disabled");

    Ok(())
}

/// Check rich logging status
pub async fn status() -> Result<()> {
    output::header("Rich Logging Status");

    docker::check_docker()?;

    let containers = [
        ("Grafana", "devdock-grafana-server", "http://localhost:3000"),
        ("Loki (Gateway)", "devdock-grafana-nginx", "http://localhost:3100"),
        ("Loki (Read)", "devdock-grafana-loki-read", "http://localhost:3101"),
        ("Loki (Write)", "devdock-grafana-loki-write", "http://localhost:3102"),
        ("Loki (Backend)", "devdock-grafana-loki-backend", ""),
        ("Alloy", "devdock-grafana-alloy", "http://localhost:12345"),
        ("Minio", "devdock-grafana-minio", ""),
        ("Prometheus", "devdock-prometheus", "http://localhost:9090"),
    ];

    let mut running = 0;

    for (name, container, url) in containers {
        let output = Command::new("docker")
            .args(["inspect", "-f", "{{.State.Running}}", container])
            .output();

        let is_running =
            output.map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true").unwrap_or(false);

        if is_running {
            if !url.is_empty() {
                output::success(&format!("✅ {name} is running: {url}"));
            } else {
                output::success(&format!("✅ {name} is running"));
            }
            running += 1;
        } else {
            output::warn(&format!("❌ {name} is not running"));
        }
    }

    println!();
    if running == containers.len() {
        output::success("All rich logging services are running");
    } else if running > 0 {
        output::warn(&format!("{}/{} services running", running, containers.len()));
    } else {
        output::info("Rich logging is not enabled. Use 'devdock logs enable' to start.");
    }

    Ok(())
}
