//! Cleanup command implementation.

use crate::output;
use anyhow::Result;
use devdock_config::docker;
use devdock_remote::ssh;
use devdock_state::{Instance, InstanceStatus, StateManager};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Ports to clean up
const CLEANUP_PORTS: &[u16] = &[8888, 8880, 7070, 8090, 7972, 3407, 3408];

/// Execute the cleanup command
pub async fn execute(soft: bool, clean_logs: bool, clean_local: bool, keep_generated: bool) -> Result<()> {
    output::header("DevDock Cleanup");

    let home = std::env::var("HOME").unwrap_or_default();
    let devdock_dir = PathBuf::from(&home).join(".devdock");
    let devdock_root = std::env::var("DEVDOCK_ROOT")
        .unwrap_or_else(|_| std::env::current_dir().unwrap().display().to_string());

    let total_steps = if clean_local { 6 } else { 4 };
    let mut step = 1;

    if clean_local {
        docker::check_docker()?;

        // Step 1: Clean up service containers
        output::step(step, total_steps, "Cleaning service containers...");
        let containers = docker::get_devdock_containers()?;
        let mut cleaned = 0;

        for container in &containers {
            // In soft mode, skip infra containers
            if soft && is_infra_container(container) {
                continue;
            }

            output::info(&format!("Removing {container}..."));
            docker::remove_container(container)?;
            cleaned += 1;
        }
        output::success(&format!("Cleaned {cleaned} containers"));
        step += 1;
    }

    // Step 2: Clean up generated files
    if keep_generated {
        output::info("Keeping generated Dockerfiles and docker-compose.yaml (use --clean-generated to remove)");
    } else {
        output::step(step, total_steps, "Cleaning generated files...");
        clean_generated_files(&devdock_root, &devdock_dir)?;
        output::success("Generated files cleaned");
    }
    step += 1;

    // Step 3: Clean up Grafana/observability stack
    output::step(step, total_steps, "Cleaning observability stack...");
    clean_grafana_stack(&devdock_dir, clean_local)?;
    output::success("Observability stack cleaned");
    step += 1;

    if clean_local {
        // Step 4: Clean up dangling images
        output::step(step, total_steps, "Cleaning dangling images...");
        let removed = docker::remove_dangling_images()?;
        if removed > 0 {
            output::success(&format!("Removed {removed} dangling images"));
        } else {
            output::info("No dangling images found");
        }
        step += 1;
    }

    // Step 5: Clean up ports
    output::step(step, total_steps, "Cleaning up ports...");
    let killed = docker::cleanup_ports(CLEANUP_PORTS)?;
    if killed > 0 {
        output::success(&format!("Killed {killed} processes on ports"));
    } else {
        output::info("No processes to kill");
    }
    step += 1;

    // Step 6: Clean up stale instance records from database
    output::step(step, total_steps, "Cleaning stale instance records...");
    match clean_stale_instances(clean_local).await {
        Ok(count) => {
            if count > 0 {
                output::success(&format!("Cleaned {count} stale instance record(s)"));
            } else {
                output::info("No stale instances found");
            }
        }
        Err(e) => {
            output::warn(&format!("Failed to clean instance records: {e}"));
        }
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
        "mysql",
        "sql",
        "redis",
        "opensearch",
        "elasticsearch",
        "memcached",
        "nginx",
        "pubsub",
        "rabbitmq",
        "servicebus",
    ];

    infra_names.iter().any(|n| name.contains(n))
}

fn clean_generated_files(devdock_root: &str, devdock_dir: &Path) -> Result<()> {
    // Files/dirs to remove
    let to_remove = [
        devdock_dir.join("opensearch/full_cleanup.txt"),
        devdock_dir.join("sql/full_cleanup.txt"),
        devdock_dir.join("resource_monitor.yaml"),
        devdock_dir.join("resource_monitor.log"),
        devdock_dir.join("startup-monitor.pid"),
        devdock_dir.join("sql/scripts"),
        devdock_dir.join("sql/util"),
        PathBuf::from(devdock_root).join("devdock/devdock.ini"),
        PathBuf::from(devdock_root).join("devdock/scio_apps.ini"),
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

    // Clean up generated files in service, pipeline, and test_suite directories
    let config_dirs = [
        PathBuf::from(devdock_root).join("devdock/config/services"),
        PathBuf::from(devdock_root).join("devdock/config/pipelines"),
        PathBuf::from(devdock_root).join("devdock/config/test_suite"),
    ];

    for config_dir in &config_dirs {
        if !config_dir.exists() {
            continue;
        }

        let is_test_suite = config_dir.ends_with("test_suite");

        for entry in std::fs::read_dir(config_dir)?.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            if is_test_suite {
                // test_suite dirs are entirely temporary — remove the whole dir
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(path.join("Dockerfile"));
                let _ = std::fs::remove_file(path.join("docker-compose.yaml"));
                let _ = std::fs::remove_file(path.join("requirements.txt"));
                let _ = std::fs::remove_dir_all(path.join("wheels"));
            }
        }
    }

    // Clean per-service state under ~/.devdock/{service}/ (preserve logs)
    clean_service_state_dirs(devdock_dir);

    Ok(())
}

/// Remove per-service state files under ~/.devdock/{service}/, preserving *.log files.
/// Matches the Python cleanup: `find ~/.devdock/{service} -type f ! -name "*.log" -delete`
fn clean_service_state_dirs(devdock_dir: &Path) {
    let skip_dirs = ["images", "auth", "sql", "grafana", "bazel", "generated", "sync", "cache"];

    let entries = match std::fs::read_dir(devdock_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        if skip_dirs.contains(&dir_name.as_str()) {
            continue;
        }

        // Delete non-log files
        for file in walkdir::WalkDir::new(&path).into_iter().filter_map(std::result::Result::ok) {
            let fp = file.path();
            if fp.is_file() && fp.extension().and_then(|e| e.to_str()) != Some("log") {
                let _ = std::fs::remove_file(fp);
            }
            if file.path_is_symlink() {
                let _ = std::fs::remove_file(fp);
            }
        }

        // Remove now-empty directories bottom-up
        for file in walkdir::WalkDir::new(&path)
            .contents_first(true)
            .into_iter()
            .filter_map(std::result::Result::ok)
        {
            if file.path().is_dir() {
                let _ = std::fs::remove_dir(file.path());
            }
        }
    }
}

fn clean_grafana_stack(devdock_dir: &Path, docker: bool) -> Result<()> {
    // Remove Grafana data
    let grafana_dir = devdock_dir.join("grafana");
    if grafana_dir.exists() {
        let _ = std::fs::remove_dir_all(&grafana_dir);
    }

    if !docker {
        return Ok(());
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

        let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if skip_dirs.contains(&dir_name) {
            continue;
        }

        // Remove log files and empty directories
        for file in walkdir::WalkDir::new(&path).into_iter().filter_map(std::result::Result::ok) {
            let file_path = file.path();
            if file_path.extension().is_some_and(|e| e == "log") {
                let _ = std::fs::remove_file(file_path);
            }
        }

        // Remove symlinks
        for file in walkdir::WalkDir::new(&path).into_iter().filter_map(std::result::Result::ok) {
            if file.path_is_symlink() {
                let _ = std::fs::remove_file(file.path());
            }
        }
    }

    Ok(())
}

/// Clean up stale instance records from the database
///
/// This removes:
/// - Instances marked as Running/Starting but without active Tilt processes
/// - Instances in Stopping state that never completed
/// - ALL Stopped and Error instances (cleanup all old instances)
async fn clean_stale_instances(clean_local: bool) -> Result<usize> {
    let state_manager = StateManager::new().await?;
    _clean_stale_instances(CleanStaleInstancesParams {
        state_manager: &state_manager,
        clean_local,
        is_tilt_running,
    })
    .await
}

struct CleanStaleInstancesParams<'a, F>
where
    F: Fn(&devdock_state::Instance) -> Result<bool>,
{
    state_manager: &'a StateManager,
    clean_local: bool,
    is_tilt_running: F,
}
async fn _clean_stale_instances<F>(params: CleanStaleInstancesParams<'_, F>) -> Result<usize>
where
    F: Fn(&devdock_state::Instance) -> Result<bool>,
{
    let CleanStaleInstancesParams { state_manager, clean_local, is_tilt_running } = params;
    let instances = state_manager.list_instances().await?;

    let mut cleaned_count = 0;

    for instance in instances {
        if !clean_local && instance.mode == "local" {
            continue;
        }

        // Check if instance should be cleaned up
        let should_clean = match instance.status {
            InstanceStatus::Running | InstanceStatus::Starting => {
                // Check if Tilt is actually running in this workspace
                !is_tilt_running(&instance)?
            }
            InstanceStatus::Stopping => {
                // Stopping instances that never completed should be cleaned
                true
            }
            InstanceStatus::Stopped | InstanceStatus::Error => {
                // Clean up all stopped/error instances
                true
            }
        };

        if should_clean {
            output::info(&format!(
                "  Removing instance {} ({})",
                Instance::display_id(&instance.id),
                instance.status.as_str()
            ));

            if let Err(e) = state_manager.delete_instance(&instance.id).await {
                output::warn(&format!(
                    "  Failed to delete instance {}: {}",
                    Instance::display_id(&instance.id),
                    e
                ));
            } else {
                cleaned_count += 1;
            }
        }
    }

    Ok(cleaned_count)
}

/// Check if Tilt is running for an instance.
fn is_tilt_running(instance: &devdock_state::Instance) -> Result<bool> {
    match instance.mode.as_str() {
        "local" => Ok(is_tilt_running_locally(&instance.workspace_path)),
        "remote" => is_tilt_running_remotely(instance),
        _ => anyhow::bail!("invalid instance mode: {}", instance.mode.as_str()),
    }
}

/// Check if Tilt is running in a specific local workspace
///
/// This checks for:
/// 1. A Tilt process with the workspace path as current directory
/// 2. Any devdock containers for this workspace
fn is_tilt_running_locally(workspace_path: &str) -> bool {
    // Check for Tilt process
    if is_process_running_in_workspace("tilt", workspace_path) {
        return true;
    }

    // Check for DevDock containers (as a backup check)
    if let Ok(containers) = docker::get_devdock_containers() {
        if !containers.is_empty() {
            // If there are DevDock containers running, consider it active
            // This handles cases where Tilt might have a different process name
            return true;
        }
    }

    false
}

fn is_tilt_running_remotely(instance: &devdock_state::Instance) -> Result<bool> {
    let workspace = instance.workspace_name.as_deref().ok_or_else(|| {
        anyhow::anyhow!("missing workspace name for remote instance {}", instance.id)
    })?;

    // Primary: Tilt UI port is open.
    if ssh::ssh_check(workspace, "lsof -i:10350 >/dev/null 2>&1", None)? {
        return Ok(true);
    }

    // Fallback: Tilt process is running.
    if ssh::ssh_check(workspace, "pgrep -f 'tilt up' >/dev/null 2>&1", None)? {
        return Ok(true);
    }

    Ok(false)
}

/// Check if a process is running in a specific workspace directory
fn is_process_running_in_workspace(process_name: &str, workspace_path: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        // Use ps to find processes with matching name
        let output = Command::new("ps").args(["-ax", "-o", "pid,command"]).output();

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);

            for line in stdout.lines() {
                if line.contains(process_name) {
                    // Extract PID from the line
                    if let Some(pid_str) = line.split_whitespace().next() {
                        if let Ok(pid) = pid_str.parse::<i32>() {
                            // Check the process's working directory
                            if let Ok(cwd) = get_process_cwd(pid) {
                                if cwd.starts_with(workspace_path) {
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Use pgrep to find processes with matching name
        let output = Command::new("pgrep").arg("-f").arg(process_name).output();

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);

            for pid_str in stdout.lines() {
                if let Ok(pid) = pid_str.parse::<i32>() {
                    if let Ok(cwd) = get_process_cwd(pid) {
                        if cwd.starts_with(workspace_path) {
                            return true;
                        }
                    }
                }
            }
        }
    }

    false
}

/// Get the current working directory of a process
#[cfg(target_os = "macos")]
fn get_process_cwd(pid: i32) -> Result<String> {
    let output =
        Command::new("lsof").args(["-p", &pid.to_string(), "-a", "-d", "cwd", "-Fn"]).output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        if let Some(stripped) = line.strip_prefix('n') {
            return Ok(stripped.to_string());
        }
    }

    anyhow::bail!("Could not determine cwd for process {pid}")
}

/// Get the current working directory of a process
#[cfg(target_os = "linux")]
fn get_process_cwd(pid: i32) -> Result<String> {
    let cwd_link = format!("/proc/{}/cwd", pid);
    let cwd = std::fs::read_link(&cwd_link)?;
    Ok(cwd.to_string_lossy().to_string())
}

/// Fallback for unsupported platforms
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn get_process_cwd(_pid: i32) -> Result<String> {
    anyhow::bail!("get_process_cwd not supported on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;
    use devdock_state::{Instance, InstanceStatus};
    use tempfile::NamedTempFile;

    async fn setup_state_manager() -> Result<(StateManager, NamedTempFile)> {
        let db_file = NamedTempFile::new()?;
        let state_manager = StateManager::with_path(db_file.path()).await?;
        Ok((state_manager, db_file))
    }

    fn make_instance(id: &str, mode: &str, status: InstanceStatus) -> Instance {
        Instance {
            id: id.to_string(),
            workspace_path: format!("/workspaces/{id}"),
            workspace_name: if mode == "remote" { Some(format!("ws-{id}")) } else { None },
            branch: "master".to_string(),
            profile: "default".to_string(),
            mode: mode.to_string(),
            status,
            port_base: 10000,
            created_at: 1,
            updated_at: 1,
        }
    }

    fn instance_ids(instances: &[Instance]) -> Vec<String> {
        let mut ids: Vec<String> = instances.iter().map(|instance| instance.id.clone()).collect();
        ids.sort();
        ids
    }

    #[tokio::test]
    async fn clean_stale_instances_keeps_running_when_tilt_running() -> Result<()> {
        let (state_manager, _db_file) = setup_state_manager().await?;
        let instance = make_instance("inst-0001", "local", InstanceStatus::Running);
        let instance_id = instance.id.clone();
        state_manager.register_instance(&instance).await?;

        let cleaned = _clean_stale_instances(CleanStaleInstancesParams {
            state_manager: &state_manager,
            clean_local: true,
            is_tilt_running: |_| Ok(true),
        })
        .await?;

        assert_eq!(cleaned, 0);
        let remaining = state_manager.list_instances().await?;
        assert_eq!(instance_ids(&remaining), vec![instance_id]);
        Ok(())
    }

    #[tokio::test]
    async fn clean_stale_instances_cleans_running_when_tilt_not_running() -> Result<()> {
        let (state_manager, _db_file) = setup_state_manager().await?;
        let instance = make_instance("inst-0001", "local", InstanceStatus::Running);
        state_manager.register_instance(&instance).await?;

        let cleaned = _clean_stale_instances(CleanStaleInstancesParams {
            state_manager: &state_manager,
            clean_local: true,
            is_tilt_running: |_| Ok(false),
        })
        .await?;

        assert_eq!(cleaned, 1);
        let remaining = state_manager.list_instances().await?;
        assert!(remaining.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn clean_stale_instances_cleans_stopping_instances() -> Result<()> {
        let (state_manager, _db_file) = setup_state_manager().await?;
        let instance = make_instance("inst-0001", "local", InstanceStatus::Stopping);
        state_manager.register_instance(&instance).await?;

        let cleaned = _clean_stale_instances(CleanStaleInstancesParams {
            state_manager: &state_manager,
            clean_local: true,
            is_tilt_running: |_| Ok(true),
        })
        .await?;

        assert_eq!(cleaned, 1);
        let remaining = state_manager.list_instances().await?;
        assert!(remaining.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn clean_stale_instances_cleans_stopped_and_error_instances() -> Result<()> {
        let (state_manager, _db_file) = setup_state_manager().await?;
        let stopped = make_instance("inst-0001", "local", InstanceStatus::Stopped);
        let errored = make_instance("inst-0002", "local", InstanceStatus::Error);
        state_manager.register_instance(&stopped).await?;
        state_manager.register_instance(&errored).await?;

        let cleaned = _clean_stale_instances(CleanStaleInstancesParams {
            state_manager: &state_manager,
            clean_local: true,
            is_tilt_running: |_| Ok(true),
        })
        .await?;

        assert_eq!(cleaned, 2);
        let remaining = state_manager.list_instances().await?;
        assert!(remaining.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn clean_stale_instances_skips_local_when_clean_local_false() -> Result<()> {
        let (state_manager, _db_file) = setup_state_manager().await?;
        let local = make_instance("inst-local", "local", InstanceStatus::Stopped);
        let remote = make_instance("inst-remote", "remote", InstanceStatus::Stopped);
        let local_id = local.id.clone();
        state_manager.register_instance(&local).await?;
        state_manager.register_instance(&remote).await?;

        let cleaned = _clean_stale_instances(CleanStaleInstancesParams {
            state_manager: &state_manager,
            clean_local: false,
            is_tilt_running: |_| Ok(false),
        })
        .await?;

        assert_eq!(cleaned, 1);
        let remaining = state_manager.list_instances().await?;
        assert_eq!(instance_ids(&remaining), vec![local_id]);
        Ok(())
    }
}
