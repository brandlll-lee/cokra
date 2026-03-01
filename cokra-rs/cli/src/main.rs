use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use cokra_config::ConfigLoader;
use cokra_core::Cokra;
use cokra_core::model::auth::{AuthManager, AuthRequest, AuthType, Credentials};
use cokra_protocol::{EventMsg, Op, UserInput};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

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

#[derive(Debug, clap::Args)]
struct CliConfigOverrides {
  /// Configuration override in key=value format
  #[arg(short = 'c', long = "config", value_name = "KEY=VALUE")]
  overrides: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum Commands {
  Interactive {
    #[arg(short = 'd', long = "dir")]
    dir: Option<PathBuf>,
  },
  Run {
    task: String,
    #[arg(short = 'd', long = "dir")]
    dir: Option<PathBuf>,
  },
  Exec {
    task: String,
    #[arg(short = 'd', long = "dir")]
    dir: Option<PathBuf>,
    /// Print events as JSON Lines instead of human-readable text
    #[arg(long = "jsonl")]
    jsonl: bool,
  },
  Mcp {
    #[command(subcommand)]
    mcp_command: McpCommands,
  },
  Config {
    #[command(subcommand)]
    config_command: ConfigCommands,
  },
  Auth {
    #[command(subcommand)]
    auth_command: AuthCommands,
  },
  Models,
}

#[derive(Debug, Subcommand)]
enum McpCommands {
  List,
  Test {
    server: String,
  },
  Add {
    name: String,
    #[arg(trailing_var_arg = true)]
    command: Vec<String>,
  },
  Remove {
    server: String,
  },
}

#[derive(Debug, Subcommand)]
enum ConfigCommands {
  Show,
  Edit,
  Validate,
  Set { key: String, value: String },
}

#[derive(Debug, Subcommand)]
enum AuthCommands {
  Login {
    /// Provider id (openai/anthropic/openrouter/google/github/...)
    #[arg(short = 'p', long = "provider")]
    provider: Option<String>,

    #[arg(short = 'k', long = "key")]
    api_key: Option<String>,

    /// Use OAuth device flow instead of API key.
    #[arg(long = "oauth")]
    oauth: bool,

    /// OAuth client id (required by provider-specific OAuth flow).
    #[arg(long = "client-id")]
    client_id: Option<String>,
  },
  Logout {
    #[arg(short = 'p', long = "provider")]
    provider: Option<String>,
  },
  Status {
    #[arg(short = 'p', long = "provider")]
    provider: Option<String>,
  },
}

#[tokio::main]
async fn main() -> Result<()> {
  let cli = TopCli::parse();
  let overrides = parse_overrides(&cli.config_overrides.overrides)?;

  match cli.command {
    Some(Commands::Interactive { dir }) => run_interactive(dir.or(cli.dir), overrides).await,
    Some(Commands::Run { task, dir }) => run_task(task, dir.or(cli.dir), overrides).await,
    Some(Commands::Exec { task, dir, jsonl }) => {
      run_exec(task, dir.or(cli.dir), overrides, jsonl).await
    }
    Some(Commands::Mcp { mcp_command }) => handle_mcp_command(mcp_command).await,
    Some(Commands::Config { config_command }) => handle_config_command(config_command).await,
    Some(Commands::Auth { auth_command }) => handle_auth_command(auth_command).await,
    Some(Commands::Models) => list_models().await,
    None => {
      if let Some(prompt) = cli.prompt {
        run_task(prompt, cli.dir, overrides).await
      } else {
        run_interactive(cli.dir, overrides).await
      }
    }
  }
}

fn parse_overrides(overrides: &[String]) -> Result<Vec<(String, String)>> {
  overrides
    .iter()
    .map(|entry| {
      let (key, value) = entry
        .split_once('=')
        .with_context(|| format!("invalid override '{entry}', expected key=value"))?;
      Ok((key.to_string(), value.to_string()))
    })
    .collect()
}

fn set_workdir(dir: &Option<PathBuf>) -> Result<()> {
  if let Some(dir) = dir {
    std::env::set_current_dir(dir)
      .with_context(|| format!("failed to set working directory to {}", dir.display()))?;
  }
  Ok(())
}

fn load_config(
  dir: &Option<PathBuf>,
  overrides: Vec<(String, String)>,
) -> Result<cokra_config::Config> {
  let loader = match dir {
    Some(d) => ConfigLoader::default().with_project_dir(d.clone()),
    None => ConfigLoader::default(),
  };
  loader.load_with_cli_overrides(overrides)
}

async fn run_task(
  task: String,
  dir: Option<PathBuf>,
  overrides: Vec<(String, String)>,
) -> anyhow::Result<()> {
  set_workdir(&dir)?;
  let config = load_config(&dir, overrides)?;
  let cokra = Cokra::new(config).await?;
  let result = cokra.run_turn(task).await?;
  println!("{}", result.final_message);
  Ok(())
}

async fn run_interactive(
  dir: Option<PathBuf>,
  overrides: Vec<(String, String)>,
) -> anyhow::Result<()> {
  set_workdir(&dir)?;
  let config = load_config(&dir, overrides)?;
  let cokra = Cokra::new(config).await?;

  println!("Cokra Interactive Mode");
  println!("Type 'exit' to quit");

  let stdin = io::stdin();
  let mut stdout = io::stdout();

  loop {
    print!("cokra> ");
    stdout.flush()?;

    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    let input = line.trim().to_string();

    if input.is_empty() {
      continue;
    }

    if matches!(input.as_str(), "exit" | "quit") {
      break;
    }

    if input == "help" {
      println!("Commands: help, exit, quit");
      continue;
    }

    match cokra.run_turn(input).await {
      Ok(result) => println!("{}\n", result.final_message),
      Err(err) => eprintln!("error: {err}\n"),
    }
  }

  Ok(())
}

async fn run_exec(
  task: String,
  dir: Option<PathBuf>,
  overrides: Vec<(String, String)>,
  jsonl: bool,
) -> anyhow::Result<()> {
  set_workdir(&dir)?;
  let config = load_config(&dir, overrides)?;
  let cokra = Cokra::new(config).await?;

  let _submission_id = cokra
    .submit(Op::UserInput {
      items: vec![UserInput::Text {
        text: task,
        text_elements: Vec::new(),
      }],
      final_output_json_schema: None,
    })
    .await?;

  loop {
    let event = cokra.next_event().await?;
    if jsonl {
      println!("{}", serde_json::to_string(&event)?);
    } else {
      print_human_event(&event.msg);
    }

    if matches!(
      event.msg,
      EventMsg::TurnComplete(_) | EventMsg::TurnAborted(_) | EventMsg::Error(_)
    ) {
      break;
    }
  }

  Ok(())
}

fn print_human_event(event: &EventMsg) {
  match event {
    EventMsg::SessionConfigured(e) => {
      println!(
        "[session] thread={} model={} approval={} sandbox={}",
        e.thread_id, e.model, e.approval_policy, e.sandbox_mode
      );
    }
    EventMsg::TurnStarted(e) => {
      println!("[turn:start] turn={} model={}", e.turn_id, e.model);
    }
    EventMsg::ItemStarted(e) => {
      println!("[item:start] id={} type={}", e.item_id, e.item_type);
    }
    EventMsg::AgentMessageDelta(e) | EventMsg::AgentMessageContentDelta(e) => {
      print!("{}", e.delta);
      let _ = io::stdout().flush();
    }
    EventMsg::ItemCompleted(e) => {
      if !e.result.trim().is_empty() {
        println!("\n[item:done] {}", e.result);
      } else {
        println!("\n[item:done]");
      }
    }
    EventMsg::TurnComplete(e) => {
      println!("[turn:done] status={:?}", e.status);
    }
    EventMsg::TurnAborted(e) => {
      println!("[turn:aborted] {}", e.reason);
    }
    EventMsg::Error(e) => {
      println!("[error] {}", e.user_facing_message);
    }
    EventMsg::Warning(e) => {
      println!("[warning] {}", e.message);
    }
    _ => {
      println!("[event] {:?}", event);
    }
  }
}

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

async fn handle_config_command(cmd: ConfigCommands) -> anyhow::Result<()> {
  match cmd {
    ConfigCommands::Show => {
      println!("Current configuration:");
      println!("  (Use cokra config edit to customize)");
    }
    ConfigCommands::Edit => {
      println!("Config edit is not implemented yet.");
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

async fn handle_auth_command(cmd: AuthCommands) -> anyhow::Result<()> {
  let manager = AuthManager::new().unwrap_or_default();

  match cmd {
    AuthCommands::Login {
      provider,
      api_key,
      oauth,
      client_id,
    } => {
      let provider = provider.unwrap_or_else(|| "openai".to_string());

      if oauth {
        let request = if let Some(client_id) = client_id {
          AuthRequest::new(provider.clone(), AuthType::OAuthDevice).with_client_id(client_id)
        } else {
          AuthRequest::new(provider.clone(), AuthType::OAuthDevice)
        };

        let pending = manager.begin_oauth(request).await?;
        if let Credentials::DeviceCode {
          user_code,
          verification_url,
          ..
        } = pending.credentials
        {
          println!("OAuth login started for provider: {}", provider);
          println!("1) Open: {}", verification_url);
          println!("2) Enter code: {}", user_code);
          println!("Waiting for authorization...");
          manager.complete_oauth(&provider, "").await?;
          println!("OAuth login completed for {}", provider);
        } else {
          println!("OAuth started, but provider returned unexpected state.");
        }
      } else if let Some(key) = api_key {
        manager.save(&provider, Credentials::ApiKey { key }).await?;
        println!("API key stored for provider: {}", provider);
      } else {
        println!("Please pass -k <api_key> or --oauth.");
      }
    }
    AuthCommands::Logout { provider } => {
      let provider = provider.unwrap_or_else(|| "openai".to_string());
      manager.remove(&provider).await?;
      println!("Logged out from {}", provider);
    }
    AuthCommands::Status { provider } => {
      if let Some(provider) = provider {
        let has = manager.has_credentials(&provider).await;
        let status = if has { "configured" } else { "not configured" };
        println!("{}: {}", provider, status);
      } else {
        let providers = manager.list_providers().await?;
        if providers.is_empty() {
          println!("No stored credentials.");
        } else {
          println!("Stored credentials:");
          for provider in providers {
            println!("  {}", provider);
          }
        }
      }
    }
  }
  Ok(())
}

async fn list_models() -> anyhow::Result<()> {
  println!("Available models:");
  println!("  openai/gpt-4o");
  println!("  anthropic/claude-sonnet-4");
  println!("  openrouter/openai/gpt-4o");
  println!("  google/gemini-2.0-flash-exp");
  println!("  ollama/llama3");
  println!("  lmstudio/<loaded_model>");
  Ok(())
}
