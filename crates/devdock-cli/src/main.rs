//! DevDock CLI entry point.

mod commands;
mod output;

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
        } => commands::start::execute(&profile, services.as_deref(), ci_mode).await,

        Commands::Stop { target, force } => commands::stop::execute(target.as_deref(), force).await,

        Commands::Status { detailed } => commands::status::execute(detailed).await,

        Commands::List { running } => commands::list::execute(running).await,

        Commands::Validate { profile } => commands::validate::execute(&profile).await,

        Commands::Generate { profile, output } => {
            commands::generate::execute(&profile, output.as_deref()).await
        }

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
    };

    // Handle result
    if let Err(e) = result {
        eprintln!("{} {}", "Error:".red().bold(), e);
        process::exit(1);
    }
}

