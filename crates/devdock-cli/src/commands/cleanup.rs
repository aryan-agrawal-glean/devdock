//! Cleanup command implementation.

use crate::output;
use anyhow::Result;
use devdock_config::docker;
use std::path::PathBuf;

/// Ports to clean up
const CLEANUP_PORTS: &[u16] = &[8888, 8880, 7070, 8090, 7972, 3407, 3408];

/// Execute the cleanup command
pub async fn execute(soft: bool, clean_logs: bool) -> Result<()> {
    output::header("DevDock Cleanup");

    // Check Docker is running
    docker::check_docker()?;

    let home = std::env::var("HOME").unwrap_or_default();
    let devdock_dir = PathBuf::from(&home).join(".devdock");
    let devdock_root = std::env::var("DEVDOCK_ROOT")
        .unwrap_or_else(|_| std::env::current_dir().unwrap().display().to_string());

    // Step 1: Clean up service containers
    output::step(1, 5, "Cleaning service containers...");
    let containers = docker::get_devdock_containers()?;
    let mut cleaned = 0;
    
    for container in &containers {
        // In soft mode, skip infra containers
        if soft && is_infra_container(container) {
            continue;
        }
        
        output::info(&format!("Removing {}...", container));
        docker::remove_container(container)?;
        cleaned += 1;
    }
    output::success(&format!("Cleaned {} containers", cleaned));

    // Step 2: Clean up generated files
    output::step(2, 5, "Cleaning generated files...");
    clean_generated_files(&devdock_root, &devdock_dir)?;
    output::success("Generated files cleaned");

    // Step 3: Clean up Grafana/observability stack
    output::step(3, 5, "Cleaning observability stack...");
    clean_grafana_stack(&devdock_dir)?;
    output::success("Observability stack cleaned");

    // Step 4: Clean up dangling images
    output::step(4, 5, "Cleaning dangling images...");
    let removed = docker::remove_dangling_images()?;
    if removed > 0 {
        output::success(&format!("Removed {} dangling images", removed));
    } else {
        output::info("No dangling images found");
    }

    // Step 5: Clean up ports
    output::step(5, 5, "Cleaning up ports...");
    let killed = docker::cleanup_ports(CLEANUP_PORTS)?;
    if killed > 0 {
        output::success(&format!("Killed {} processes on ports", killed));
    } else {
        output::info("No processes to kill");
    }

    // Optional: Clean logs
    if clean_logs {
        output::info("Cleaning old logs...");
        clean_old_logs(&devdock_dir)?;
        output::success("Old logs cleaned");
    }

    output::header("Cleanup Complete");
    output::success("✅ DevDock cleanup finished");
    output::info("💡 Tip: Use 'devdock cleanup --logs' to also clean old log files");

    Ok(())
}

fn is_infra_container(name: &str) -> bool {
    let infra_names = [
        "mysql", "sql", "redis", "opensearch", "elasticsearch",
        "memcached", "nginx", "pubsub", "rabbitmq", "servicebus",
    ];
    
    infra_names.iter().any(|n| name.contains(n))
}

fn clean_generated_files(devdock_root: &str, devdock_dir: &PathBuf) -> Result<()> {
    // Files/dirs to remove
    let to_remove = [
        devdock_dir.join("opensearch/full_cleanup.txt"),
        devdock_dir.join("sql/full_cleanup.txt"),
        devdock_dir.join("resource_monitor.yaml"),
        devdock_dir.join("resource_monitor.log"),
        devdock_dir.join("sql/scripts"),
        devdock_dir.join("sql/util"),
        PathBuf::from(devdock_root).join("devdock/devdock.ini"),
        PathBuf::from(devdock_root).join("devdock/runtime_generated"),
        PathBuf::from(devdock_root).join("config/devdock.ini"),
        PathBuf::from(devdock_root).join("config/run_local.ini"),
    ];

    for path in &to_remove {
        if path.exists() {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    // Clean up service directories
    let services_dir = PathBuf::from(devdock_root).join("devdock/config/services");
    if services_dir.exists() {
        for entry in std::fs::read_dir(&services_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                // Remove generated files in service dirs
                let _ = std::fs::remove_file(path.join("Dockerfile"));
                let _ = std::fs::remove_file(path.join("docker-compose.yaml"));
                let _ = std::fs::remove_file(path.join("requirements.txt"));
                let _ = std::fs::remove_dir_all(path.join("wheels"));
            }
        }
    }

    Ok(())
}

fn clean_grafana_stack(devdock_dir: &PathBuf) -> Result<()> {
    // Remove Grafana data
    let grafana_dir = devdock_dir.join("grafana");
    if grafana_dir.exists() {
        let _ = std::fs::remove_dir_all(&grafana_dir);
    }

    // Stop and remove Grafana containers
    let grafana_containers = ["devdock-grafana", "devdock-loki", "devdock-alloy", "devdock-minio"];
    for container in grafana_containers {
        docker::remove_container(container)?;
    }

    Ok(())
}

fn clean_old_logs(devdock_dir: &PathBuf) -> Result<()> {
    // Skip special directories
    let skip_dirs = ["images", "auth", "sql", "grafana", "bazel"];

    if !devdock_dir.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(devdock_dir)? {
        let entry = entry?;
        let path = entry.path();
        
        if !path.is_dir() {
            continue;
        }

        let dir_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if skip_dirs.contains(&dir_name) {
            continue;
        }

        // Remove log files and empty directories
        for file in walkdir::WalkDir::new(&path)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let file_path = file.path();
            if file_path.extension().map(|e| e == "log").unwrap_or(false) {
                let _ = std::fs::remove_file(file_path);
            }
        }

        // Remove symlinks
        for file in walkdir::WalkDir::new(&path)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if file.path_is_symlink() {
                let _ = std::fs::remove_file(file.path());
            }
        }
    }

    Ok(())
}


