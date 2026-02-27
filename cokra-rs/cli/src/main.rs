# Cokra CLI Entry Point
# Command-line interface for Cokra AI Agent Team CLI

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::info;

#[derive(Parser)]
#[command(name = "cokra")]
#[command(about = "Cokra - AI Agent Team CLI Environment", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start interactive mode
    Interactive {
        /// Working directory
        #[arg(short, long)]
        dir: Option<String>,
    },
    /// Run a single task
    Run {
        /// Task description
        task: String,
    },
    /// Show configuration
    Config {
        #[command(subcommand)]
        config_command: ConfigCommands,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Show current configuration
    Show,
    /// Validate configuration
    Validate,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "info".to_string())
                .as_str(),
        )
        .init();

    let cli = Cli::parse();

    info!("Cokra CLI starting...");

    match cli.command {
        Commands::Interactive { dir } => {
            info!("Starting interactive mode");
            if let Some(dir) = info!("Working directory: {}", dir);
            // TODO: Launch TUI
            println!("Interactive mode - coming soon");
        }
        Commands::Run { task } => {
            info!("Running task: {}", task);
            // TODO: Execute single task
            println!("Task execution - coming soon");
        }
        Commands::Config { config_command } => match config_command {
            ConfigCommands::Show => {
                println!("Configuration: coming soon");
            }
            ConfigCommands::Validate => {
                println!("Configuration validation: coming soon");
            }
        },
    }

    Ok(())
}
