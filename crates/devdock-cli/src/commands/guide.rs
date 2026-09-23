//! Intent-based command guidance for humans and agents.

use anyhow::Result;

use crate::output;
use crate::GuideIntent;

pub async fn execute(intent: Option<GuideIntent>) -> Result<()> {
    match intent {
        Some(GuideIntent::RemoteStart) => remote_start(),
        Some(GuideIntent::AddService) => add_service(),
        Some(GuideIntent::SqlHydration) => sql_hydration(),
        Some(GuideIntent::FileSync) => file_sync(),
        Some(GuideIntent::DockerAuth) => docker_auth(),
        Some(GuideIntent::Troubleshoot) => troubleshoot(),
        Some(GuideIntent::Cleanup) => cleanup(),
        None => overview(),
    }

    Ok(())
}

fn overview() {
    output::header("DevDock Guide");
    println!("Use `devdock guide <intent>` when you know what you want to do but not which flags to use. This command is intentionally stable so humans and agents can discover workflows without scraping every --help output.\n");
    println!("Common intents:");
    println!("  remote-start    Start or reuse a remote Coder-backed DevDock");
    println!("  add-service     Add or change services in a running DevDock");
    println!("  sql-hydration   Hydrate or repair local SQL data");
    println!("  file-sync       Inspect or force remote file sync");
    println!("  docker-auth     Fix Docker/GCR/Artifact Registry auth issues");
    println!("  troubleshoot    Collect status and report a broken session");
    println!("  cleanup         Recover from stale local DevDock state safely\n");
    println!("Examples:");
    println!("  devdock guide remote-start");
    println!("  devdock guide add-service");
    println!("  devdock guide troubleshoot");
}

fn remote_start() {
    output::header("Guide: start remote DevDock");
    println!("Intent: start DevDock in a remote Coder workspace.\n");
    println!("Recommended flow:");
    println!("  1. devdock setup --remote");
    println!("  2. devdock start <profile> --remote --auto-forward");
    println!("  3. devdock status -d");
    println!("  4. devdock sync status");
    println!("     devdock sync history -n 20\n");
    println!("Equivalent legacy form:");
    println!("  devdock start <profile> -m remote --auto-forward\n");
    println!("Useful options:");
    println!("  --branch <branch>          Use a specific branch for the remote workspace");
    println!("  --workspace-name <name>    Use a stable Coder workspace name");
    println!("  --disk-size <gb>           Set Coder data disk size");
    println!("  --port-base <port>         Offset forwarded local ports");
    println!(
        "  --copy-git-creds           Copy local gh auth to a new or explicitly selected --vm workspace (opt-in)"
    );
    println!(
        "\nWithout --copy-git-creds, remote workspaces keep the default automation credentials."
    );
    println!("Credential transfer uses legacy scp -O transport.");
    println!("\nAuthentication and validation:");
    println!("  - If local gcloud/ADC expires, ask the user to run locally: gcloud auth login --update-adc");
    println!("    Then retry remote start so DevDock can sync the refreshed credentials.");
    println!("  - Never run interactive gcloud auth in the remote VM.");
    println!("  - Validate with the explicit candidate binary and a native Linux build in Coder.");
    println!(
        "  - Inspect remote logs, status, and sync history; --monitor alone is not validation."
    );
}

fn add_service() {
    output::header("Guide: add/change services");
    println!("Intent: update services without tearing down the whole DevDock.\n");
    println!("Current safe workflow:");
    println!("  devdock profile apply --services <svc1,svc2,...>");
    println!("  devdock ports forward --all      # remote only, if ports changed");
    println!("  devdock status -d\n");
    println!("Notes:");
    println!("  - Include all services you want active today; `profile apply` preserves current active services only when --services is omitted.");
    println!("  - `devdock service enable <name>` is reserved for the hot-add workflow and is not implemented yet.");
}

fn sql_hydration() {
    output::header("Guide: SQL hydration");
    println!("Intent: hydrate or reset local SQL data.\n");
    println!("Common commands:");
    println!("  devdock infra sql setup --project <project> --mode fast");
    println!("  devdock infra sql setup --project <project> --mode latest");
    println!("  devdock infra sql clean --project <project>\n");
    println!("Useful options:");
    println!("  --db-tables <spec>        Limit copied DBs/tables");
    println!("  --force-copy              Re-copy even if state says data is current");
    println!("  --full-cleanup            Recreate local SQL state before hydration");
    println!("  --use-mysql-seed          Use pre-seeded MySQL data dir when available");
}

fn file_sync() {
    output::header("Guide: remote file sync");
    println!("Intent: understand or repair local -> remote file sync.\n");
    println!("Common commands:");
    println!("  devdock sync status");
    println!("  devdock sync history -n 20");
    println!("  devdock sync errors -n 20");
    println!("  devdock sync force <path> [--watch]\n");
    println!("Notes:");
    println!("  - Remote start auto-starts sync.");
    println!("  - Sync is primarily local -> remote.");
    println!("  - Use `sync force` for files outside the normal watched patterns.");
}

fn docker_auth() {
    output::header("Guide: Docker auth");
    println!("Intent: fix Docker image pull/auth failures.\n");
    println!("Common commands:");
    println!("  devdock docker check");
    println!("  devdock docker login");
    println!("  devdock docker pull\n");
    println!("If using Google Artifact Registry/GCR:");
    println!("  gcloud auth login");
    println!("  gcloud auth application-default login");
    println!("  gcloud auth configure-docker");
}

fn troubleshoot() {
    output::header("Guide: troubleshoot DevDock");
    println!("Intent: inspect a broken session and collect enough context for support.\n");
    println!("Start here:");
    println!("  devdock status -d");
    println!("  devdock sync status");
    println!("  devdock sync errors -n 20");
    println!("  devdock docker check\n");
    println!("If the issue is not obvious:");
    println!("  devdock report --summary \"<short summary>\" --description \"<what happened>\" --command \"<command you ran>\"");
}

fn cleanup() {
    output::header("Guide: cleanup/recovery");
    println!("Intent: recover from stale containers, generated files, or local state without deleting ~/.devdock.\n");
    println!("Common commands:");
    println!("  devdock cleanup --soft          # service containers only");
    println!("  devdock cleanup --logs          # include old logs");
    println!("  devdock cleanup --clean-generated\n");
    println!("Avoid `rm -rf ~/.devdock` unless a DevDock maintainer explicitly asks for it; it removes state that targeted cleanup should preserve.");
}
