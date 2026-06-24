//! DevDock CLI entry point.

mod commands;
mod logging;
mod output;

use clap::{ArgAction, Parser, Subcommand};
use colored::Colorize;
use std::process;

#[derive(Parser)]
#[command(name = "devdock")]
#[command(version = concat!(env!("CARGO_PKG_VERSION"), env!("DEV_VERSION_SUFFIX")), about = "Standalone development environment orchestrator", long_about = None)]
struct Cli {
    /// Increase logging verbosity (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start DevDock with a profile
    Start {
        /// Profile name (e.g., default_profile)
        profile: String,

        /// Specific services to start (comma-separated)
        #[arg(short, long)]
        services: Option<String>,

        /// CI mode (non-interactive)
        #[arg(long)]
        ci_mode: bool,

        /// Start mode: local or remote
        #[arg(short = 'm', long, default_value = "local")]
        mode: String,

        /// Use existing Coder workspace
        #[arg(long)]
        vm: Option<String>,

        /// Custom workspace name (default: branch name)
        #[arg(long)]
        workspace_name: Option<String>,

        /// Target branch for remote workspace (default: current behavior)
        #[arg(long)]
        branch: Option<String>,

        /// GCP region for Coder workspace
        #[arg(long, default_value = "us-east4-a")]
        region: String,

        /// Custom port base (remote only)
        #[arg(long)]
        port_base: Option<u16>,

        /// Data disk size in GB for Coder workspace (default: 300)
        #[arg(long)]
        disk_size: Option<u32>,

        /// Auto-forward all service ports (opt-in for remote)
        #[arg(long)]
        auto_forward: bool,

        /// Show detailed progress (all substeps)
        #[arg(long)]
        verbose: bool,

        /// Enable startup monitor to wait for all resources to become healthy
        #[arg(long)]
        monitor: bool,

        /// Startup monitor timeout in seconds (default: 1200 = 20 minutes)
        #[arg(long)]
        monitor_timeout: Option<u64>,

        /// Disable telemetry/metrics export to BigQuery
        #[arg(long)]
        no_telemetry: bool,

        /// Dry-run mode: execute all startup logic but don't start Tilt (for benchmarking)
        #[arg(long)]
        dry_run: bool,

        /// Enable rich logging (Grafana/Loki stack) before starting
        #[arg(long)]
        enable_logs: bool,
    },

    /// Stop DevDock instance
    Stop {
        /// Instance ID or service name
        target: Option<String>,

        /// Force stop without cleanup
        #[arg(short, long)]
        force: bool,

        /// Delete the Coder workspace (non-interactive)
        #[arg(long)]
        delete: bool,
    },

    /// Show DevDock status
    Status {
        /// Show detailed status
        #[arg(short, long)]
        detailed: bool,
    },

    /// List all DevDock instances
    List {
        /// Show only running instances
        #[arg(short, long)]
        running: bool,
    },

    /// Validate a profile
    Validate {
        /// Profile name to validate
        profile: String,
        /// Also validate all devdock_config.json files can be parsed
        #[arg(long)]
        strict: bool,
    },

    /// Generate configuration without starting
    Generate {
        /// Profile name
        profile: String,

        /// Output directory
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Full cleanup of containers, networks, and files
    #[command(alias = "-c")]
    Cleanup {
        /// Only clean service containers (not infra)
        #[arg(long)]
        soft: bool,

        /// Also clean old logs
        #[arg(long)]
        logs: bool,

        /// Also clean generated Dockerfiles and docker-compose.yaml (default: preserved for faster restarts)
        #[arg(long)]
        clean_generated: bool,

        /// Skip local cleanup steps and only clenaup remotes (default: false)
        #[arg(long = "no-local")]
        no_local: bool,
    },

    /// Execute command in a container
    Exec {
        /// Container name (e.g., qe, sql, mysql)
        container: String,

        /// Command to execute (default: sh, mysql for sql container)
        #[arg(default_value = "")]
        command: String,
    },

    /// Docker management (images, networks, etc.)
    Docker {
        #[command(subcommand)]
        command: DockerCommands,
    },

    /// Initial setup of DevDock
    Setup {
        /// Setup for remote DevDock (Coder workspaces)
        #[arg(long)]
        remote: bool,
    },

    /// Service management
    Service {
        #[command(subcommand)]
        command: ServiceCommands,
    },

    /// Profile management
    Profile {
        #[command(subcommand)]
        command: ProfileCommands,
    },

    /// Plugin management
    Plugin {
        #[command(subcommand)]
        command: PluginCommands,
    },

    /// Infrastructure setup (SQL, OpenSearch, etc.)
    Infra {
        #[command(subcommand)]
        command: InfraCommands,
    },

    /// Rich logging management (Grafana/Loki stack)
    Logs {
        #[command(subcommand)]
        command: LogsCommands,
    },

    /// Port management for remote DevDock
    Ports {
        #[command(subcommand)]
        command: PortCommands,
    },

    /// File sync management (for remote DevDock)
    Sync {
        #[command(subcommand)]
        command: SyncCommands,
    },

    /// Report a bug to Jira (interactive)
    Report {
        /// Brief summary of the issue (skip interactive prompt)
        #[arg(long)]
        summary: Option<String>,

        /// Detailed description of the issue (skip interactive prompt)
        #[arg(long)]
        description: Option<String>,

        /// Command that was run when the issue occurred (skip interactive prompt)
        #[arg(long)]
        command: Option<String>,
    },

    /// Monitor startup resources until all healthy (or timeout)
    Monitor {
        /// Timeout in seconds (default: 1200 = 20 minutes)
        #[arg(short, long)]
        timeout: Option<u64>,

        /// Polling interval in seconds (default: 2)
        #[arg(short, long)]
        interval: Option<u64>,

        /// Disable desktop notifications
        #[arg(long)]
        no_notify: bool,

        /// Profile name (for telemetry)
        #[arg(long)]
        profile: Option<String>,
    },

    /// Update devdock to latest or specific version
    Update {
        /// Version to install (e.g., '0.2.7', '@0.2.7', or 'latest')
        /// Omit to update to latest version
        /// Using a specific version will pin it
        version: Option<String>,

        /// Only check for updates without installing
        #[arg(long)]
        check: bool,

        /// Force update even if pinned
        #[arg(long)]
        force: bool,

        /// List available versions
        #[arg(long)]
        list: bool,
    },
}

#[derive(Subcommand)]
enum ServiceCommands {
    /// Enable a service
    Enable { name: String },
    /// Disable a service
    Disable { name: String },
    /// View service logs
    Logs {
        name: String,
        #[arg(short, long)]
        follow: bool,
    },
}

#[derive(Subcommand)]
enum ProfileCommands {
    /// List available profiles
    List,
    /// Show profile details
    Show { name: String },
    /// Apply profile changes to the running DevDock without tearing it down
    Apply {
        /// Profile name to apply. Defaults to the running instance's profile.
        #[arg(long)]
        profile: Option<String>,

        /// Target instance ID. Defaults to the instance for the current workspace.
        #[arg(long)]
        instance: Option<String>,

        /// Active services to preserve/use (comma-separated). Defaults to current temp_profile active services.
        #[arg(short, long)]
        services: Option<String>,

        /// Apply in the current environment only; used internally by remote apply and Tilt helpers.
        #[arg(long, hide = true)]
        local_only: bool,

        /// Preview target/delta without writing files or triggering resources.
        #[arg(long)]
        dry_run: bool,

        /// Do not trigger sql-hydrator even when local SQL is configured.
        #[arg(long)]
        no_sql_refresh: bool,

        /// Skip confirmation prompts, including backing project switches.
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Create a new profile interactively
    Create,
}

#[derive(Subcommand)]
enum PluginCommands {
    /// List installed plugins
    List,
    /// Install a plugin
    Install { name: String },
    /// Remove a plugin
    Remove { name: String },
}

#[derive(Subcommand)]
enum PortCommands {
    /// List port mappings (alias for compatibility)
    List {
        /// Show ports for specific instance ID
        #[arg(long)]
        instance: Option<String>,
    },
    /// Enable auto port forwarding for remote DevDock
    Forward {
        /// Forward all common service ports
        #[arg(long)]
        all: bool,
        /// Forward specific service port (e.g., qe, fe, tilt)
        #[arg(long)]
        service: Option<String>,
        /// Custom port mapping in format local:remote
        #[arg(long)]
        custom: Option<String>,
        /// Don't apply port_base offset to local ports (use remote port as local port)
        #[arg(long)]
        no_offset: bool,
    },
    /// Stop port forwarding
    Stop {
        /// Stop all port forwards
        #[arg(long)]
        all: bool,
        /// Stop specific service port forward
        #[arg(long)]
        service: Option<String>,
    },
}

#[derive(Subcommand)]
enum DockerCommands {
    /// Check if Docker is running
    Check,

    /// Setup Docker networks
    Networks,

    /// Pull/update Docker images
    Pull {
        /// Only pull Python images
        #[arg(long)]
        python_only: bool,

        /// Save images to cache after pulling
        #[arg(long)]
        cache: bool,
    },

    /// Login to Google Container Registry
    Login,

    /// Remove dangling images
    Prune,
}

#[derive(Subcommand)]
enum LogsCommands {
    /// Enable rich logging (Grafana/Loki/Alloy stack)
    Enable,

    /// Disable rich logging and cleanup
    Disable,

    /// Show rich logging status
    Status,
}

#[derive(Subcommand)]
enum SyncCommands {
    /// Start file sync (auto-started with remote mode)
    Start {
        /// Sync all files instead of git-tracked only
        #[arg(long)]
        full: bool,
    },

    /// Stop file sync
    Stop,

    /// Show sync status
    Status,

    /// Show sync history
    History {
        /// Number of recent entries to show
        #[arg(long, short = 'n')]
        tail: Option<usize>,
    },

    /// Show sync errors
    Errors {
        /// Number of recent errors to show
        #[arg(long, short = 'n')]
        tail: Option<usize>,
    },

    /// Force-sync files/directories that bypass normal filters
    Force {
        /// File paths, directory paths, or glob patterns to sync
        #[arg(required = true)]
        paths: Vec<String>,

        /// Workspace name (standalone mode, otherwise auto-detected from running instance)
        #[arg(long)]
        workspace: Option<String>,

        /// Also add patterns to the daemon's ongoing watch list
        #[arg(long)]
        watch: bool,
    },
}

#[derive(Subcommand)]
enum InfraCommands {
    /// SQL infrastructure management
    Sql {
        #[command(subcommand)]
        command: InfraSqlCommands,
    },

    /// OpenSearch infrastructure management
    Opensearch {
        #[command(subcommand)]
        command: InfraOpenSearchCommands,
    },
}

#[derive(Subcommand)]
enum InfraSqlCommands {
    /// Full SQL setup (replaces sql_hydrator.sh + util.py)
    Setup {
        /// Project name
        #[arg(long)]
        project: String,

        /// Platform (google, aws, azure)
        #[arg(long, default_value = "google")]
        platform: String,

        /// Region
        #[arg(long, default_value = "us-central1")]
        region: String,

        /// Database/tables list (format: db:tables:query;db2:tables2)
        #[arg(long, default_value = "")]
        db_tables: String,

        /// Mode: fast (hourly snapshot) or latest
        #[arg(long, default_value = "fast")]
        mode: String,

        /// Force copy even if tables match
        #[arg(long)]
        force_copy: bool,

        /// Full cleanup before setup
        #[arg(long)]
        full_cleanup: bool,

        /// Copy keyval table (default: true)
        #[arg(long, default_value_t = true, action = ArgAction::Set)]
        copy_keyval: bool,

        /// Use pre-seeded MySQL data directory (fastest startup)
        #[arg(long)]
        use_mysql_seed: bool,

        /// Use full instance dump instead of table-by-table
        #[arg(long)]
        use_full_instance: bool,

        /// Use GAC container (devdock-mysql-gac) for secondary GAC project setup
        #[arg(long)]
        use_gac_container: bool,
    },

    /// Clean SQL databases/tables
    Clean {
        /// Project name
        #[arg(long)]
        project: String,

        /// Platform (google, aws, azure)
        #[arg(long, default_value = "google")]
        platform: String,

        /// Region
        #[arg(long, default_value = "us-central1")]
        region: String,

        /// Database/tables to clean (empty = all non-system DBs)
        #[arg(long, default_value = "")]
        db_tables: String,
    },
}

#[derive(Subcommand)]
enum InfraOpenSearchCommands {
    /// Copy indexes from remote OpenSearch to local
    Copy {
        /// Project name
        #[arg(long)]
        project: String,

        /// Indexes to copy (comma-separated)
        #[arg(long)]
        indexes: String,

        /// Query to filter documents
        #[arg(long, default_value = "{\"match_all\": {}}")]
        query: String,

        /// Maximum documents per index (0 = unlimited)
        #[arg(long, default_value = "0")]
        max_docs: usize,
    },

    /// Check local OpenSearch health
    Health,
}

#[tokio::main]
async fn main() {
    // Check if we're being called as the sync daemon (hidden internal command)
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 5 && args[1] == "__sync_daemon__" {
        // Run as sync daemon process
        let devdock_id = &args[2];
        let workspace_name = &args[3];
        let repo_path = std::path::PathBuf::from(&args[4]);
        let full_mode = args.get(5).is_some_and(|s| s == "true");
        let branch_mode = args.get(6).is_some_and(|s| s == "true");
        let log_level = args.get(7).map_or("info", |s| s.as_str());

        // Stderr is already redirected to the log file by the parent process,
        // so initializing tracing with the default stderr writer routes all
        // tracing output (debug!, info!, warn!, error!) into the log file.
        tracing_subscriber::fmt().with_env_filter(log_level).with_target(false).init();

        if let Err(e) = devdock_sync::daemon::run_daemon_process(
            devdock_id,
            workspace_name,
            &repo_path,
            full_mode,
            branch_mode,
        ) {
            eprintln!("Daemon process error: {e}");
            process::exit(1);
        }
        process::exit(0);
    }

    let cli = Cli::parse();

    // Check for updates on startup (unless running update command itself)
    if !matches!(cli.command, Commands::Update { .. }) {
        let _ = commands::update::check_on_startup().await;
    }

    // Set up logging
    let log_level = match cli.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    tracing_subscriber::fmt().with_env_filter(log_level).with_target(false).init();

    // Run command
    let result = match cli.command {
        Commands::Start {
            profile,
            services,
            ci_mode,
            mode,
            vm,
            workspace_name,
            branch,
            region,
            port_base,
            disk_size,
            auto_forward,
            verbose,
            monitor,
            monitor_timeout,
            no_telemetry,
            dry_run,
            enable_logs,
        } => {
            if mode == "remote" {
                commands::remote::start_remote(commands::remote::RemoteStartConfig {
                    profile,
                    workspace_name,
                    branch,
                    region,
                    port_base,
                    disk_size,
                    vm,
                    auto_forward,
                    services,
                    verbose,
                    log_level: devdock_sync::daemon::verbosity_to_log_level(cli.verbose)
                        .to_string(),
                })
                .await
            } else {
                commands::start::execute(
                    &profile,
                    services.as_deref(),
                    ci_mode,
                    monitor, // monitoring disabled by default (opt-in with --monitor)
                    monitor_timeout,
                    !no_telemetry, // telemetry enabled by default
                    dry_run,
                    enable_logs,
                )
                .await
            }
        }

        Commands::Stop { target, force, delete } => {
            commands::stop::execute(target.as_deref(), force, delete).await
        }

        Commands::Status { detailed } => commands::status::execute(detailed).await,

        Commands::List { running } => commands::list::execute(running).await,

        Commands::Validate { profile, strict } => {
            commands::validate::execute(&profile, strict).await
        }

        Commands::Generate { profile, output } => {
            commands::generate::execute(&profile, output.as_deref()).await
        }

        Commands::Cleanup { soft, logs, clean_generated, no_local } => {
            commands::cleanup::execute(soft, logs, !no_local, !clean_generated).await
        }

        Commands::Exec { container, command } => {
            commands::exec::execute(&container, &command).await
        }

        Commands::Docker { command } => match command {
            DockerCommands::Check => commands::docker::check().await,
            DockerCommands::Networks => commands::docker::networks().await,
            DockerCommands::Pull { python_only, cache } => {
                commands::docker::pull(python_only, cache).await
            }
            DockerCommands::Login => commands::docker::login().await,
            DockerCommands::Prune => commands::docker::prune().await,
        },

        Commands::Setup { remote } => commands::setup::execute(remote).await,

        Commands::Logs { command } => match command {
            LogsCommands::Enable => commands::logs::enable().await,
            LogsCommands::Disable => commands::logs::disable().await,
            LogsCommands::Status => commands::logs::status().await,
        },

        Commands::Ports { command } => match command {
            PortCommands::List { instance } => commands::ports::list(instance).await,
            PortCommands::Forward { all, service, custom, no_offset } => {
                commands::ports::forward(all, service.as_deref(), custom.as_deref(), no_offset)
                    .await
            }
            PortCommands::Stop { all, service } => {
                commands::ports::stop(all, service.as_deref()).await
            }
        },

        Commands::Sync { command } => match command {
            SyncCommands::Start { full } => {
                let log_level = devdock_sync::daemon::verbosity_to_log_level(cli.verbose);
                commands::sync::start(full, log_level).await
            }
            SyncCommands::Stop => commands::sync::stop().await,
            SyncCommands::Status => commands::sync::status().await,
            SyncCommands::History { tail } => commands::sync::history(tail).await,
            SyncCommands::Errors { tail } => commands::sync::errors(tail).await,
            SyncCommands::Force { paths, workspace, watch } => {
                commands::sync::force(paths, workspace, watch).await
            }
        },

        Commands::Report { summary, description, command } => match (summary, description, command)
        {
            (Some(s), Some(d), Some(c)) => commands::report::report_bug_with_details(s, d, c).await,
            _ => commands::report::report_bug().await,
        },

        Commands::Monitor { timeout, interval, no_notify, profile } => {
            commands::monitor::execute(timeout, interval, no_notify, profile).await
        }

        Commands::Update { version, check, force, list } => {
            commands::update::execute(version, check, force, list).await
        }

        Commands::Service { command } => match command {
            ServiceCommands::Enable { name } => commands::service::enable(&name).await,
            ServiceCommands::Disable { name } => commands::service::disable(&name).await,
            ServiceCommands::Logs { name, follow } => commands::service::logs(&name, follow).await,
        },

        Commands::Profile { command } => match command {
            ProfileCommands::List => commands::profile::list().await,
            ProfileCommands::Show { name } => commands::profile::show(&name).await,
            ProfileCommands::Apply {
                profile,
                instance,
                services,
                local_only,
                dry_run,
                no_sql_refresh,
                yes,
            } => {
                commands::profile::apply(commands::profile::ApplyOptions {
                    profile,
                    instance,
                    services,
                    local_only,
                    dry_run,
                    no_sql_refresh,
                    yes,
                })
                .await
            }
            ProfileCommands::Create => commands::profile::create().await,
        },

        Commands::Plugin { command } => match command {
            PluginCommands::List => commands::plugin::list().await,
            PluginCommands::Install { name } => commands::plugin::install(&name).await,
            PluginCommands::Remove { name } => commands::plugin::remove(&name).await,
        },

        Commands::Infra { command } => match command {
            InfraCommands::Sql { command } => match command {
                InfraSqlCommands::Setup {
                    project,
                    platform,
                    region,
                    db_tables,
                    mode,
                    force_copy,
                    full_cleanup,
                    copy_keyval,
                    use_mysql_seed,
                    use_full_instance,
                    use_gac_container,
                } => {
                    commands::infra::sql_setup(
                        &project,
                        &platform,
                        &region,
                        &db_tables,
                        &mode,
                        force_copy,
                        full_cleanup,
                        copy_keyval,
                        use_mysql_seed,
                        use_full_instance,
                        use_gac_container,
                    )
                    .await
                }
                InfraSqlCommands::Clean { project, platform, region, db_tables } => {
                    commands::infra::sql_clean(&project, &platform, &region, &db_tables).await
                }
            },

            InfraCommands::Opensearch { command } => match command {
                InfraOpenSearchCommands::Copy { project, indexes, query, max_docs } => {
                    commands::infra::opensearch_copy(&project, &indexes, &query, max_docs).await
                }
                InfraOpenSearchCommands::Health => commands::infra::opensearch_health().await,
            },
        },
    };

    // Handle result
    if let Err(e) = result {
        eprintln!("{} {}", "Error:".red().bold(), e);
        process::exit(1);
    }
}
