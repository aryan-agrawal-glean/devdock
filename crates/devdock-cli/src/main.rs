//! DevDock CLI entry point.

mod commands;
mod output;
mod logging;

use clap::{Parser, Subcommand};
use colored::Colorize;
use std::process;

#[derive(Parser)]
#[command(name = "devdock")]
#[command(version, about = "Standalone development environment orchestrator", long_about = None)]
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

        /// GCP region for Coder workspace
        #[arg(long, default_value = "us-east4-a")]
        region: String,

        /// Custom port base (remote only)
        #[arg(long)]
        port_base: Option<u16>,

        /// Auto-forward all service ports (opt-in for remote)
        #[arg(long)]
        auto_forward: bool,
    },

    /// Stop DevDock instance
    Stop {
        /// Instance ID or service name
        target: Option<String>,

        /// Force stop without cleanup
        #[arg(short, long)]
        force: bool,
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

    /// Show port mappings for remote DevDock
    Ports {
        /// Show ports for specific instance ID
        #[arg(long)]
        instance: Option<String>,
    },

    /// File sync management (for remote DevDock)
    Sync {
        #[command(subcommand)]
        command: SyncCommands,
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
        #[arg(long, default_value = "true")]
        copy_keyval: bool,
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
    let cli = Cli::parse();

    // Set up logging
    let log_level = match cli.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    tracing_subscriber::fmt()
        .with_env_filter(log_level)
        .with_target(false)
        .init();

    // Run command
    let result = match cli.command {
        Commands::Start {
            profile,
            services,
            ci_mode,
            mode,
            vm,
            workspace_name,
            region,
            port_base,
            auto_forward,
        } => {
            if mode == "remote" {
                commands::remote::start_remote(
                    &profile,
                    workspace_name,
                    &region,
                    port_base,
                    vm,
                    auto_forward,
                    services.as_deref(),
                ).await
            } else {
                commands::start::execute(&profile, services.as_deref(), ci_mode).await
            }
        },

        Commands::Stop { target, force } => commands::stop::execute(target.as_deref(), force).await,

        Commands::Status { detailed } => commands::status::execute(detailed).await,

        Commands::List { running } => commands::list::execute(running).await,

        Commands::Validate { profile } => commands::validate::execute(&profile).await,

        Commands::Generate { profile, output } => {
            commands::generate::execute(&profile, output.as_deref()).await
        }

        Commands::Cleanup { soft, logs } => commands::cleanup::execute(soft, logs).await,

        Commands::Exec { container, command } => commands::exec::execute(&container, &command).await,

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

        Commands::Ports { instance } => commands::ports::execute(instance).await,

        Commands::Sync { command } => match command {
            SyncCommands::Start { full } => commands::sync::start(full).await,
            SyncCommands::Stop => commands::sync::stop().await,
            SyncCommands::Status => commands::sync::status().await,
        },

        Commands::Service { command } => match command {
            ServiceCommands::Enable { name } => commands::service::enable(&name).await,
            ServiceCommands::Disable { name } => commands::service::disable(&name).await,
            ServiceCommands::Logs { name, follow } => {
                commands::service::logs(&name, follow).await
            }
        },

        Commands::Profile { command } => match command {
            ProfileCommands::List => commands::profile::list().await,
            ProfileCommands::Show { name } => commands::profile::show(&name).await,
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
                    )
                    .await
                }
                InfraSqlCommands::Clean {
                    project,
                    platform,
                    region,
                    db_tables,
                } => commands::infra::sql_clean(&project, &platform, &region, &db_tables).await,
            },

            InfraCommands::Opensearch { command } => match command {
                InfraOpenSearchCommands::Copy {
                    project,
                    indexes,
                    query,
                    max_docs,
                } => commands::infra::opensearch_copy(&project, &indexes, &query, max_docs).await,
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
