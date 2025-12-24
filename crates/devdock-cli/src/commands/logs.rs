//! Rich logging management (Grafana/Loki/Alloy stack).

use crate::output;
use anyhow::{Context, Result};
use devdock_config::docker;
use std::path::PathBuf;
use std::process::Command;

/// Docker compose file for Grafana stack
const GRAFANA_COMPOSE: &str = r#"
version: '3.8'
services:
  loki:
    container_name: devdock-grafana-loki
    image: grafana/loki:latest
    ports:
      - "3100:3100"
    networks:
      - devdock_infra
    volumes:
      - ${HOME}/.devdock/grafana/.data/loki:/loki

  alloy:
    container_name: devdock-grafana-alloy
    image: grafana/alloy:latest
    ports:
      - "12345:12345"
    networks:
      - devdock_infra
    depends_on:
      - loki

  minio:
    container_name: devdock-grafana-minio
    image: minio/minio
    ports:
      - "9000:9000"
      - "9001:9001"
    environment:
      MINIO_ROOT_USER: minioadmin
      MINIO_ROOT_PASSWORD: minioadmin
    command: server /data --console-address ":9001"
    networks:
      - devdock_infra
    volumes:
      - ${HOME}/.devdock/grafana/.data/minio:/data

  grafana:
    container_name: devdock-grafana
    image: grafana/grafana:latest
    ports:
      - "3000:3000"
    environment:
      - GF_AUTH_ANONYMOUS_ENABLED=true
      - GF_AUTH_ANONYMOUS_ORG_ROLE=Admin
    networks:
      - devdock_infra
    depends_on:
      - loki
    volumes:
      - ${HOME}/.devdock/grafana/.data/grafana:/var/lib/grafana

networks:
  devdock_infra:
    external: true
"#;

/// Enable rich logging stack
pub async fn enable() -> Result<()> {
    output::header("Enabling Rich Logging");
    
    // Check Docker
    docker::check_docker()?;
    
    // Setup networks
    output::step(1, 4, "Setting up Docker networks...");
    docker::setup_networks()?;
    output::success("Networks ready");
    
    // Create directories
    output::step(2, 4, "Creating data directories...");
    let home = std::env::var("HOME")?;
    let grafana_dir = PathBuf::from(&home).join(".devdock/grafana/.data");
    std::fs::create_dir_all(grafana_dir.join("loki"))?;
    std::fs::create_dir_all(grafana_dir.join("minio"))?;
    std::fs::create_dir_all(grafana_dir.join("grafana"))?;
    
    // Set permissions (for Linux compatibility)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o775);
        let _ = std::fs::set_permissions(&grafana_dir, perms);
    }
    output::success("Directories created");
    
    // Load cached images or pull
    output::step(3, 4, "Loading images...");
    for image in docker::RICH_LOGGING_IMAGES {
        if !docker::load_cached_image(image)? {
            output::info(&format!("Pulling {}...", image));
            docker::pull_images(&[image], false)?;
        }
    }
    output::success("Images ready");
    
    // Write compose file and start services
    output::step(4, 4, "Starting Grafana stack...");
    let compose_path = grafana_dir.parent().unwrap().join("docker-compose.yaml");
    std::fs::write(&compose_path, GRAFANA_COMPOSE)?;
    
    let status = Command::new("docker")
        .args(["compose", "-f", compose_path.to_str().unwrap(), "up", "-d"])
        .status()
        .context("Failed to start Grafana stack")?;
    
    if !status.success() {
        // Try docker-compose (older syntax)
        let status = Command::new("docker-compose")
            .args(["-f", compose_path.to_str().unwrap(), "up", "-d"])
            .status()
            .context("Failed to start Grafana stack")?;
        
        if !status.success() {
            anyhow::bail!("Failed to start Grafana stack");
        }
    }
    
    output::success("Grafana stack started");
    
    output::header("Rich Logging Enabled");
    output::success("✅ Rich logging is now available!");
    println!();
    output::info("📊 Grafana UI:    http://localhost:3000/explore");
    output::info("📊 Loki:          http://localhost:3100");
    output::info("📊 Minio Console: http://localhost:9001");
    println!();
    output::info("💡 Use Grafana to explore logs from all DevDock services");
    
    Ok(())
}

/// Disable rich logging stack
pub async fn disable() -> Result<()> {
    output::header("Disabling Rich Logging");
    
    docker::check_docker()?;
    
    // Stop containers
    output::step(1, 2, "Stopping Grafana stack...");
    let containers = [
        "devdock-grafana",
        "devdock-grafana-loki",
        "devdock-grafana-alloy",
        "devdock-grafana-minio",
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
        ("Grafana", "devdock-grafana", "http://localhost:3000"),
        ("Loki", "devdock-grafana-loki", "http://localhost:3100"),
        ("Alloy", "devdock-grafana-alloy", "http://localhost:12345"),
        ("Minio", "devdock-grafana-minio", "http://localhost:9001"),
    ];
    
    let mut running = 0;
    
    for (name, container, url) in containers {
        let output = Command::new("docker")
            .args(["inspect", "-f", "{{.State.Running}}", container])
            .output();
        
        let is_running = output
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true")
            .unwrap_or(false);
        
        if is_running {
            output::success(&format!("✅ {} is running: {}", name, url));
            running += 1;
        } else {
            output::warn(&format!("❌ {} is not running", name));
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


