// Cokra CLI - Command Line Interface Entry Point

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::info;

/// Cokra - AI Agent Team CLI Environment
#[derive(Parser, Debug)]
#[command(name = "cokra")]
#[command(version, about, long_about = None)]
struct TopCli {
    #[clap(flatten)]
    config_overrides: CliConfigOverrides,

    #[clap(subcommand)]
    command: Option<Commands>,

    /// Initial prompt/task to execute
    #[arg(short = 'p', long = "prompt")]
    prompt: Option<String>,

    /// Working directory
    #[arg(short = 'd', long = "dir")]
    dir: Option<PathBuf>,
}

/// CLI configuration overrides
#[derive(Debug, clap::Args)]
struct CliConfigOverrides {
    /// Configuration override in key=value format
    #[arg(short = 'c', long = "config", value_name = "KEY=VALUE")]
    overrides: Vec<String>,
}

/// Available commands
#[derive(Debug, Subcommand)]
enum Commands {
    /// Start interactive mode (default)
    Interactive {
        /// Working directory
        #[arg(short = 'd', long = "dir")]
        dir: Option<PathBuf>,
    },

    /// Execute a single task
    Run {
        /// Task description
        task: String,

        /// Working directory
        #[arg(short = 'd', long = "dir")]
        dir: Option<PathBuf>,
    },

    /// Manage MCP servers
    Mcp {
        #[command(subcommand)]
        mcp_command: McpCommands,
    },

    /// Configuration management
    Config {
        #[command(subcommand)]
        config_command: ConfigCommands,
    },

    /// Authentication
    Auth {
        #[command(subcommand)]
        auth_command: AuthCommands,
    },

    /// List available models
    Models,
}

/// MCP server commands
#[derive(Debug, Subcommand)]
enum McpCommands {
    /// List all MCP servers
    List,

    /// Test MCP server connection
    Test {
        /// Server name
        server: String,
    },

    /// Add a new MCP server
    Add {
        /// Server name
        name: String,

        /// Command to run
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
    },

    /// Remove an MCP server
    Remove {
        /// Server name
        server: String,
    },
}

/// Configuration commands
#[derive(Debug, Subcommand)]
enum ConfigCommands {
    /// Show current configuration
    Show,

    /// Edit configuration
    Edit,

    /// Validate configuration
    Validate,

    /// Set a configuration value
    Set {
        /// Configuration key
        key: String,

        /// Configuration value
        value: String,
    },
}

/// Authentication commands
#[derive(Debug, Subcommand)]
enum AuthCommands {
    /// Login with API key
    Login {
        /// API key
        #[arg(short = 'k', long = "key")]
        api_key: Option<String>,
    },

    /// Logout
    Logout,

    /// Show current authentication status
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "info".to_string())
                .as_str(),
        )
        .init();

    // Parse CLI arguments
    let cli = TopCli::parse();

    info!("Cokra CLI starting...");

    // Load configuration (simplified)
    let config = cokra_config::Config::default();

    // Handle different modes
    match cli.command {
        Some(Commands::Interactive { dir }) => {
            run_interactive(dir.or(cli.dir)).await?;
        }
        Some(Commands::Run { task, dir }) => {
            run_task(task, dir.or(cli.dir)).await?;
        }
        Some(Commands::Mcp { mcp_command }) => {
            handle_mcp_command(mcp_command).await?;
        }
        Some(Commands::Config { config_command }) => {
            handle_config_command(config_command).await?;
        }
        Some(Commands::Auth { auth_command }) => {
            handle_auth_command(auth_command).await?;
        }
        Some(Commands::Models) => {
            list_models().await?;
        }
        None => {
            // Default: interactive mode or run with prompt
            if let Some(prompt) = cli.prompt {
                run_task(prompt, cli.dir).await?;
            } else {
                run_interactive(cli.dir).await?;
            }
        }
    }

    Ok(())
}

/// Run interactive mode
async fn run_interactive(dir: Option<PathBuf>) -> anyhow::Result<()> {
    // Set working directory
    if let Some(dir) = dir {
        std::env::set_current_dir(&dir)?;
    }

    info!("Starting interactive mode");

    // TODO: Run TUI
    println!("Cokra Interactive Mode");
    println!("Type 'exit' to quit");
    println!();

    // Simple REPL for now
    run_repl().await
}

/// Run a single task
async fn run_task(task: String, dir: Option<PathBuf>) -> anyhow::Result<()> {
    // Set working directory
    if let Some(dir) = dir {
        std::env::set_current_dir(&dir)?;
    }

    info!("Running task: {}", task);

    // TODO: Create Cokra instance and execute
    println!("Executing: {}", task);
    println!("(Task execution coming soon)");

    Ok(())
}

/// Handle MCP commands
async fn handle_mcp_command(cmd: McpCommands) -> anyhow::Result<()> {
    match cmd {
        McpCommands::List => {
            println!("MCP Servers:");
            println!("  (No servers configured)");
        }
        McpCommands::Test { server } => {
            println!("Testing MCP server: {}", server);
        }
        McpCommands::Add { name, command } => {
            println!("Adding MCP server: {} -> {:?}", name, command);
        }
        McpCommands::Remove { server } => {
            println!("Removing MCP server: {}", server);
        }
    }
    Ok(())
}

/// Handle config commands
async fn handle_config_command(cmd: ConfigCommands) -> anyhow::Result<()> {
    match cmd {
        ConfigCommands::Show => {
            println!("Current configuration:");
            println!("  (Default configuration)");
        }
        ConfigCommands::Edit => {
            println!("Opening config editor...");
        }
        ConfigCommands::Validate => {
            println!("Configuration is valid.");
        }
        ConfigCommands::Set { key, value } => {
            println!("Setting {} = {}", key, value);
        }
    }
    Ok(())
}

/// Handle auth commands
async fn handle_auth_command(cmd: AuthCommands) -> anyhow::Result<()> {
    match cmd {
        AuthCommands::Login { api_key } => {
            if let Some(key) = api_key {
                println!("Logging in with API key...");
            } else {
                println!("Please provide an API key with -k option");
            }
        }
        AuthCommands::Logout => {
            println!("Logging out...");
        }
        AuthCommands::Status => {
            println!("Authentication status: Not logged in");
        }
    }
    Ok(())
}

/// List available models
async fn list_models() -> anyhow::Result<()> {
    println!("Available models:");
    println!();
    println!("  OpenAI:");
    println!("    gpt-4o         - GPT-4 Optimized");
    println!("    gpt-4-turbo    - GPT-4 Turbo");
    println!("    gpt-3.5-turbo  - GPT-3.5 Turbo");
    println!();
    println!("  Anthropic:");
    println!("    claude-sonnet-4     - Claude Sonnet 4");
    println!("    claude-3.5-sonnet   - Claude 3.5 Sonnet");
    println!("    claude-3-opus       - Claude 3 Opus");
    println!();
    println!("  Local (Ollama):");
    println!("    ollama/llama3       - Llama 3");
    println!("    ollama/codellama    - Code Llama");
    println!();
    println!("  Local (LM Studio):");
    println!("    lmstudio/*          - Any loaded model");
    println!();
    println!("Use: cokra -c model=<model_id> to select a model");

    Ok(())
}

/// Simple REPL
async fn run_repl() -> anyhow::Result<()> {
    use std::io::{self, BufRead, Write};

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        print!("cokra> ");
        stdout.flush()?;

        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        let line = line.trim();

        if line.is_empty() {
            continue;
        }

        if line == "exit" || line == "quit" {
            println!("Goodbye!");
            break;
        }

        if line == "help" {
            println!("Commands:");
            println!("  help     - Show this help");
            println!("  exit     - Exit Cokra");
            println!("  quit     - Exit Cokra");
            println!();
            println!("Or type any message to chat with the AI.");
            continue;
        }

        // TODO: Send to Cokra
        println!("You said: {}", line);
        println!("(AI responses coming soon)");
        println!();
    }

    Ok(())
}
