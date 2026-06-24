//! Profile management commands.

use crate::commands::start;
use crate::output;
use anyhow::{anyhow, Context, Result};
use devdock_config::{ProfileLoader, ProfileValidator};
use devdock_core::profile::Profile;
use devdock_core::types::DeploymentType;
use devdock_remote::config_sync;
use devdock_state::{Instance, InstanceStatus, StateManager};
use dialoguer::Confirm;
use serde_yaml::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct ApplyOptions {
    pub profile: Option<String>,
    pub instance: Option<String>,
    pub services: Option<String>,
    pub local_only: bool,
    pub dry_run: bool,
    pub no_sql_refresh: bool,
    pub yes: bool,
}

#[derive(Debug, Clone, Default)]
struct AppliedProfileSnapshot {
    project: Option<String>,
    platform: Option<String>,
}

pub async fn list() -> Result<()> {
    output::info("Available profiles:");

    let loader = ProfileLoader::new();
    let profiles = loader.list_profiles()?;

    if profiles.is_empty() {
        output::warn("No profiles found");
        return Ok(());
    }

    for profile in profiles {
        println!("  • {profile}");
    }

    Ok(())
}

pub async fn show(name: &str) -> Result<()> {
    let loader = ProfileLoader::new();
    let profile = loader.load(name)?;

    output::header(&format!("Profile: {name}"));
    println!("Project: {}", profile.project);
    println!("Platform: {:?}", profile.platform);

    if !profile.infra.is_empty() {
        output::header("Infrastructure");
        for (name, config) in &profile.infra {
            println!("  • {} ({:?})", name, config.deployment_type);
        }
    }

    if !profile.services.is_empty() {
        output::header("Services");
        for (name, config) in &profile.services {
            println!("  • {} ({:?})", name, config.deployment_type);
        }
    }

    Ok(())
}

pub async fn create() -> Result<()> {
    output::warn("Profile create not yet implemented - coming soon!");
    Ok(())
}

/// Apply profile changes to the running DevDock session without tearing it down.
pub async fn apply(options: ApplyOptions) -> Result<()> {
    if options.local_only {
        return apply_local_only(options, None).await;
    }

    let state = StateManager::new().await?;
    let instance = resolve_instance(&state, options.instance.as_deref()).await?;

    ensure_running(&instance)?;

    if instance.mode == "remote" {
        apply_remote(options, state, instance).await
    } else {
        apply_local_only(options, Some((state, instance))).await
    }
}

async fn apply_local_only(
    options: ApplyOptions,
    state_instance: Option<(StateManager, Instance)>,
) -> Result<()> {
    output::info("Applying profile update...");

    let workspace_root = start::find_workspace_root()?;
    let temp_profile_path = workspace_root.join("devdock/runtime_generated/temp_profile.yaml");
    let old_snapshot = read_applied_profile_snapshot(&temp_profile_path).unwrap_or_default();

    let loader = ProfileLoader::new();
    let default_profile = state_instance
        .as_ref()
        .map(|(_, instance)| instance.profile.clone())
        .or_else(|| read_current_profile_from_temp(&temp_profile_path));
    let profile_ref = options
        .profile
        .clone()
        .or(default_profile)
        .ok_or_else(|| anyhow!("Could not determine profile. Pass --profile <name>."))?;

    let (profile, profile_path) = load_profile_reference(&loader, &profile_ref)?;
    validate_profile(&profile, options.dry_run)?;

    let new_snapshot = AppliedProfileSnapshot {
        project: Some(profile.project.clone()),
        platform: Some(profile.platform.to_string()),
    };
    print_profile_delta(&old_snapshot, &new_snapshot);

    if should_confirm_project_switch(&old_snapshot, &new_snapshot, options.yes, options.local_only)
        && !Confirm::new()
            .with_prompt("Continue with backing project switch?")
            .default(false)
            .interact()?
    {
        anyhow::bail!("Profile apply cancelled");
    }

    let services = options
        .services
        .clone()
        .or_else(|| derive_active_services(&temp_profile_path).ok().flatten());

    if let Some(ref services) = services {
        output::info(&format!("Preserving active services: {services}"));
    }

    if let Some((_, instance)) = &state_instance {
        std::env::set_var("DEVDOCK_ID", &instance.id);
        std::env::set_var("DEVDOCK_PORT_OFFSET", instance.port_base.to_string());
    } else if std::env::var("DEVDOCK_ID").is_err() {
        output::warn(
            "DEVDOCK_ID is not set; generated service env will not have session correlation",
        );
    }

    if options.dry_run {
        output::info("Dry-run: generating config in memory only");
        start::generate_configs_rust(
            &workspace_root,
            &profile,
            services.as_deref(),
            Some(&profile_path.to_string_lossy()),
            true,
        )
        .await?;
        output::success("Dry-run complete; no files were changed");
        return Ok(());
    }

    maybe_set_gcloud_project(&profile)?;

    start::generate_configs_rust(
        &workspace_root,
        &profile,
        services.as_deref(),
        Some(&profile_path.to_string_lossy()),
        false,
    )
    .await?;

    if !options.no_sql_refresh && is_local_sql(&profile) {
        trigger_sql_hydrator();
    }

    if let Some((state, instance)) = state_instance {
        if profile_ref != instance.profile {
            state.update_profile(&instance.id, &profile_ref).await?;
        }
    }

    output::success("Profile update applied");
    println!("  Tilt should reload changed config files automatically.");
    println!("  If you added services or changed ports, verify Tilt/port mappings before use.");

    Ok(())
}

async fn apply_remote(
    options: ApplyOptions,
    state: StateManager,
    instance: Instance,
) -> Result<()> {
    let workspace_name = resolve_remote_workspace(&state, &instance).await?;
    let profile_name = options.profile.clone().unwrap_or_else(|| instance.profile.clone());

    let loader = ProfileLoader::new();
    let (profile, profile_path) = loader.load_with_path(&profile_name)?;
    validate_profile(&profile, options.dry_run)?;

    let old_snapshot = read_remote_applied_profile_snapshot(&workspace_name).unwrap_or_default();
    let new_snapshot = AppliedProfileSnapshot {
        project: Some(profile.project.clone()),
        platform: Some(profile.platform.to_string()),
    };
    print_profile_delta(&old_snapshot, &new_snapshot);

    if should_confirm_project_switch(&old_snapshot, &new_snapshot, options.yes, false)
        && !Confirm::new()
            .with_prompt("Continue with remote backing project switch?")
            .default(false)
            .interact()?
    {
        anyhow::bail!("Profile apply cancelled");
    }

    if options.dry_run {
        output::success(&format!(
            "Dry-run complete; would sync profile {} and apply it on remote workspace {}",
            profile_file_label(&profile_path),
            workspace_name
        ));
        return Ok(());
    }

    output::info(&format!(
        "Applying profile {} on remote workspace {}",
        profile_name, workspace_name
    ));
    println!("  → Uploading profile {}", profile_file_label(&profile_path));
    config_sync::sync_profile_file(&workspace_name, &profile_path, false)?;
    println!("  ✓ Profile uploaded");

    let mut remote_args = vec![
        "profile".to_string(),
        "apply".to_string(),
        "--local-only".to_string(),
        "--yes".to_string(),
        "--profile".to_string(),
        profile_name.clone(),
    ];
    if let Some(services) = &options.services {
        remote_args.push("--services".to_string());
        remote_args.push(services.clone());
    }
    if options.no_sql_refresh {
        remote_args.push("--no-sql-refresh".to_string());
    }

    let remote_cmd = format!(
        "cd /home/workspace/scio && export DEVDOCK_ID={} && export DEVDOCK_PORT_OFFSET={} && devdock {}",
        shell_quote(&instance.id),
        instance.port_base,
        remote_args.iter().map(|arg| shell_quote(arg)).collect::<Vec<_>>().join(" ")
    );

    let spinner =
        output::create_spinner("Regenerating remote config without restarting DevDock...");
    let remote_output =
        Command::new("coder").arg("ssh").arg(&workspace_name).arg("--").arg(&remote_cmd).output();
    spinner.finish_and_clear();
    let output = remote_output.with_context(|| {
        format!("failed to run profile apply on remote workspace {workspace_name}")
    })?;

    if !output.status.success() {
        anyhow::bail!(
            "Remote profile apply failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    print_remote_output(&output.stdout, &output.stderr);

    if profile_name != instance.profile {
        state.update_profile(&instance.id, &profile_name).await?;
    }

    output::success(&format!(
        "Profile {} applied on remote workspace {}",
        profile_name, workspace_name
    ));
    println!(
        "  If you added services or changed exposed ports, run `devdock ports forward --all`."
    );

    Ok(())
}

async fn resolve_instance(state: &StateManager, instance_id: Option<&str>) -> Result<Instance> {
    if let Some(id) = instance_id {
        return state
            .get_instance(id)
            .await?
            .ok_or_else(|| anyhow!("No DevDock instance found with id {id}"));
    }

    let workspace_path = std::env::current_dir()?.display().to_string();
    state
        .get_instance_by_workspace(&workspace_path)
        .await?
        .ok_or_else(|| anyhow!("No DevDock instance found for current workspace. Pass --instance."))
}

fn ensure_running(instance: &Instance) -> Result<()> {
    if matches!(instance.status, InstanceStatus::Running | InstanceStatus::Starting) {
        Ok(())
    } else {
        anyhow::bail!(
            "Instance {} is not running (status: {:?})",
            Instance::display_id(&instance.id),
            instance.status
        )
    }
}

async fn resolve_remote_workspace(state: &StateManager, instance: &Instance) -> Result<String> {
    if let Some(workspace_name) = &instance.workspace_name {
        if !workspace_name.is_empty() {
            return Ok(workspace_name.clone());
        }
    }

    if let Some((workspace_name, _, _, _)) = state.get_sync_session(&instance.id).await? {
        if !workspace_name.is_empty() {
            return Ok(workspace_name);
        }
    }

    anyhow::bail!("Could not determine remote Coder workspace for instance {}", instance.id)
}

fn load_profile_reference(loader: &ProfileLoader, profile_ref: &str) -> Result<(Profile, PathBuf)> {
    let path = PathBuf::from(profile_ref);
    if path.exists() {
        let profile = loader.load_from_path(&path)?;
        Ok((profile, path))
    } else {
        Ok(loader.load_with_path(profile_ref)?)
    }
}

fn validate_profile(profile: &Profile, _dry_run: bool) -> Result<()> {
    // Do not validate that the current gcloud project already matches here: applying a
    // profile is allowed to switch the backing project, and `maybe_set_gcloud_project`
    // performs that rebind before config generation.
    let validator = ProfileValidator::new();
    validator.validate(profile)?;
    Ok(())
}

fn read_current_profile_from_temp(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_yaml::from_str(&content).ok()?;
    value.get("current_profile")?.as_str().map(ToString::to_string)
}

fn read_applied_profile_snapshot(path: &Path) -> Result<AppliedProfileSnapshot> {
    let content = std::fs::read_to_string(path)?;
    let value: Value = serde_yaml::from_str(&content)?;
    Ok(AppliedProfileSnapshot {
        project: value.get("project").and_then(Value::as_str).map(ToString::to_string),
        platform: value.get("platform").and_then(Value::as_str).map(ToString::to_string),
    })
}

fn read_remote_applied_profile_snapshot(workspace_name: &str) -> Result<AppliedProfileSnapshot> {
    let cmd = "python3 - <<'PY'\nimport yaml\nfrom pathlib import Path\np=Path('/home/workspace/scio/devdock/runtime_generated/temp_profile.yaml')\nif p.exists():\n  data=yaml.safe_load(p.read_text()) or {}\n  print(data.get('project',''))\n  print(data.get('platform',''))\nPY";
    let output =
        Command::new("coder").arg("ssh").arg(workspace_name).arg("--").arg(cmd).output()?;
    if !output.status.success() {
        return Ok(AppliedProfileSnapshot::default());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    Ok(AppliedProfileSnapshot {
        project: lines.next().filter(|s| !s.is_empty()).map(ToString::to_string),
        platform: lines.next().filter(|s| !s.is_empty()).map(ToString::to_string),
    })
}

fn derive_active_services(path: &Path) -> Result<Option<String>> {
    let content = std::fs::read_to_string(path)?;
    let value: Value = serde_yaml::from_str(&content)?;
    let Some(services) = value.get("services").and_then(Value::as_mapping) else {
        return Ok(None);
    };

    let mut active = Vec::new();
    for (name, config) in services {
        let Some(name) = name.as_str() else { continue };
        let is_active = config.get("active").and_then(Value::as_bool).unwrap_or(true);
        if is_active {
            active.push(name.to_string());
        }
    }
    active.sort();

    if active.is_empty() {
        Ok(None)
    } else {
        Ok(Some(active.join(",")))
    }
}

fn print_profile_delta(old: &AppliedProfileSnapshot, new: &AppliedProfileSnapshot) {
    if old.project != new.project || old.platform != new.platform {
        output::warn("Detected backing cloud context change:");
        println!(
            "  Project:  {} -> {}",
            old.project.as_deref().unwrap_or("unknown"),
            new.project.as_deref().unwrap_or("unknown")
        );
        println!(
            "  Platform: {} -> {}",
            old.platform.as_deref().unwrap_or("unknown"),
            new.platform.as_deref().unwrap_or("unknown")
        );
    }
}

fn should_confirm_project_switch(
    old: &AppliedProfileSnapshot,
    new: &AppliedProfileSnapshot,
    yes: bool,
    local_only: bool,
) -> bool {
    !yes && !local_only
        && old.project.is_some()
        && new.project.is_some()
        && old.project != new.project
}

fn maybe_set_gcloud_project(profile: &Profile) -> Result<()> {
    let platform = profile.platform.to_string();
    if platform != "google" && platform != "gcp" {
        return Ok(());
    }

    output::info(&format!("Setting gcloud project to {}", profile.project));
    let output = Command::new("gcloud")
        .args(["config", "set", "project", &profile.project])
        .output()
        .context("failed to run gcloud config set project")?;

    if !output.status.success() {
        anyhow::bail!(
            "Failed to set gcloud project to {}: {}",
            profile.project,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

fn is_local_sql(profile: &Profile) -> bool {
    match profile.infra.get("sql") {
        Some(sql) => sql.deployment_type == DeploymentType::Local,
        None => true,
    }
}

fn trigger_sql_hydrator() {
    output::info("Triggering local SQL refresh (sql-hydrator)...");
    match Command::new("tilt").args(["trigger", "sql-hydrator"]).output() {
        Ok(output) if output.status.success() => output::success("sql-hydrator triggered"),
        Ok(output) => output::warn(&format!(
            "Failed to trigger sql-hydrator: {}",
            String::from_utf8_lossy(&output.stderr)
        )),
        Err(e) => output::warn(&format!("Failed to run tilt trigger sql-hydrator: {e}")),
    }
}

fn profile_file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToString::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn print_remote_output(stdout: &[u8], stderr: &[u8]) {
    let stdout = String::from_utf8_lossy(stdout);
    if !stdout.trim().is_empty() {
        println!("{}", stdout.trim());
    }

    let stderr = String::from_utf8_lossy(stderr);
    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim());
    }
}
