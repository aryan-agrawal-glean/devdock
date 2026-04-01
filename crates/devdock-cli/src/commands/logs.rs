//! Rich logging management — available as external plugin.
//!
//! The Grafana/Loki stack is not part of the core DevDock binary.
//! Set it up as an external plugin or use docker-compose directly.

use crate::output;
use anyhow::Result;

pub async fn enable() -> Result<()> {
    output::info("Rich logging (Grafana/Loki) is available as an external plugin.");
    println!();
    println!("  To set up Grafana/Loki logging:");
    println!("    1. Add a docker-compose.yaml for Grafana to your repo");
    println!("    2. Or use: docker-compose -f path/to/grafana/docker-compose.yaml up -d");
    println!();
    println!("  The Grafana stack is no longer bundled with DevDock core.");
    Ok(())
}

pub async fn disable() -> Result<()> {
    output::info("Stopping Grafana containers...");
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
        let _ = std::process::Command::new("docker").args(["rm", "-f", container]).output();
    }
    output::success("Grafana containers stopped");
    Ok(())
}

pub async fn status() -> Result<()> {
    output::info("Rich logging status:");
    let containers = [
        ("Grafana", "devdock-grafana-server"),
        ("Loki", "devdock-grafana-nginx"),
        ("Alloy", "devdock-grafana-alloy"),
    ];

    for (name, container) in containers {
        let output = std::process::Command::new("docker")
            .args(["inspect", "-f", "{{.State.Running}}", container])
            .output();
        let running =
            output.map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true").unwrap_or(false);
        if running {
            output::success(&format!("{name} is running"));
        } else {
            output::warn(&format!("{name} is not running"));
        }
    }
    Ok(())
}
