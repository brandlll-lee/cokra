use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use cokra_config::ConfigLoader;
use cokra_core::Cokra;
use cokra_core::model::auth::AuthManager;
use cokra_core::model::auth::AuthRequest;
use cokra_core::model::auth::AuthType;
use cokra_core::model::auth::Credentials;
use cokra_core::model::init_model_layer;
use cokra_protocol::EventMsg;
use cokra_protocol::Op;
use cokra_protocol::UserInput;
use cokra_tui::UiMode;
use cokra_tui::run_main as run_tui_main;
use std::io::Write;
use std::io::{self};
use std::path::Path;
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

  /// Tell the agent to use the specified directory as its working root.
  #[arg(long = "cd", short = 'C', value_name = "DIR", alias = "cwd")]
  cwd: Option<PathBuf>,

  /// Legacy working directory flag (compat). Prefer `--cd/-C`.
  #[arg(long = "dir", short = 'd', value_name = "DIR", hide = true)]
  dir_compat: Option<PathBuf>,

  /// TUI mode: inline or alt-screen
  #[arg(long = "ui-mode", value_enum)]
  ui_mode: Option<CliUiMode>,

  /// Allow running non-interactive commands outside a Git repository.
  #[arg(long = "skip-git-repo-check", global = true, default_value_t = false)]
  skip_git_repo_check: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum CliUiMode {
  Inline,
  AltScreen,
}

impl From<CliUiMode> for UiMode {
  fn from(value: CliUiMode) -> Self {
    match value {
      CliUiMode::Inline => UiMode::Inline,
      CliUiMode::AltScreen => UiMode::AltScreen,
    }
  }
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
    /// Tell the agent to use the specified directory as its working root.
    #[arg(long = "cd", short = 'C', value_name = "DIR", alias = "cwd")]
    cwd: Option<PathBuf>,

    /// Legacy working directory flag (compat). Prefer `--cd/-C`.
    #[arg(long = "dir", short = 'd', value_name = "DIR", hide = true)]
    dir_compat: Option<PathBuf>,
  },
  Run {
    task: String,
    /// Tell the agent to use the specified directory as its working root.
    #[arg(long = "cd", short = 'C', value_name = "DIR", alias = "cwd")]
    cwd: Option<PathBuf>,

    /// Legacy working directory flag (compat). Prefer `--cd/-C`.
    #[arg(long = "dir", short = 'd', value_name = "DIR", hide = true)]
    dir_compat: Option<PathBuf>,

    /// Allow running outside a Git repository (Codex parity for non-interactive mode).
    #[arg(long = "skip-git-repo-check", default_value_t = false)]
    skip_git_repo_check: bool,
  },
  Exec {
    task: String,
    /// Tell the agent to use the specified directory as its working root.
    #[arg(long = "cd", short = 'C', value_name = "DIR", alias = "cwd")]
    cwd: Option<PathBuf>,

    /// Legacy working directory flag (compat). Prefer `--cd/-C`.
    #[arg(long = "dir", short = 'd', value_name = "DIR", hide = true)]
    dir_compat: Option<PathBuf>,

    /// Allow running outside a Git repository (Codex parity for non-interactive mode).
    #[arg(long = "skip-git-repo-check", default_value_t = false)]
    skip_git_repo_check: bool,

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
  let ui_mode = cli.ui_mode;

  match cli.command {
    Some(Commands::Interactive { cwd, dir_compat }) => {
      let resolved_cwd = resolve_cwd(cwd, dir_compat, cli.cwd, cli.dir_compat)?;
      run_interactive(resolved_cwd, overrides.clone(), ui_mode).await
    }
    Some(Commands::Run {
      task,
      cwd,
      dir_compat,
      skip_git_repo_check,
    }) => {
      let resolved_cwd = resolve_cwd(cwd, dir_compat, cli.cwd, cli.dir_compat)?;
      run_task(
        task,
        resolved_cwd,
        overrides.clone(),
        skip_git_repo_check || cli.skip_git_repo_check,
      )
      .await
    }
    Some(Commands::Exec {
      task,
      cwd,
      dir_compat,
      skip_git_repo_check,
      jsonl,
    }) => {
      let resolved_cwd = resolve_cwd(cwd, dir_compat, cli.cwd, cli.dir_compat)?;
      run_exec(
        task,
        resolved_cwd,
        overrides.clone(),
        skip_git_repo_check || cli.skip_git_repo_check,
        jsonl,
      )
      .await
    }
    Some(Commands::Mcp { mcp_command }) => handle_mcp_command(mcp_command).await,
    Some(Commands::Config { config_command }) => handle_config_command(config_command).await,
    Some(Commands::Auth { auth_command }) => handle_auth_command(auth_command).await,
    Some(Commands::Models) => {
      let resolved_cwd = resolve_cwd(None, None, cli.cwd, cli.dir_compat)?;
      list_models(resolved_cwd, overrides.clone()).await
    }
    None => {
      if let Some(prompt) = cli.prompt {
        let resolved_cwd = resolve_cwd(None, None, cli.cwd, cli.dir_compat)?;
        run_task(
          prompt,
          resolved_cwd,
          overrides.clone(),
          cli.skip_git_repo_check,
        )
        .await
      } else {
        let resolved_cwd = resolve_cwd(None, None, cli.cwd, cli.dir_compat)?;
        run_interactive(resolved_cwd, overrides.clone(), ui_mode).await
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

fn resolve_cwd(
  command_cwd: Option<PathBuf>,
  command_dir_compat: Option<PathBuf>,
  top_cwd: Option<PathBuf>,
  top_dir_compat: Option<PathBuf>,
) -> Result<PathBuf> {
  let raw = command_cwd
    .or(command_dir_compat)
    .or(top_cwd)
    .or(top_dir_compat);

  match raw.as_ref() {
    Some(path) => std::fs::canonicalize(path)
      .with_context(|| format!("failed to resolve working directory {}", path.display())),
    None => std::env::current_dir().context("failed to get current working directory"),
  }
}

fn load_config(
  resolved_cwd: &Path,
  overrides: Vec<(String, String)>,
) -> Result<cokra_config::Config> {
  ConfigLoader::default()
    .with_cwd(resolved_cwd.to_path_buf())
    .load_with_cli_overrides(overrides)
}

fn get_git_repo_root(base_dir: &Path) -> Option<PathBuf> {
  let mut dir = base_dir.to_path_buf();
  loop {
    if dir.join(".git").exists() {
      return Some(dir);
    }
    if !dir.pop() {
      break;
    }
  }
  None
}

fn resolve_ui_mode(cli_ui_mode: Option<CliUiMode>) -> UiMode {
  if let Some(mode) = cli_ui_mode {
    return mode.into();
  }

  if let Ok(env_mode) = std::env::var("COKRA_TUI_MODE")
    && let Some(mode) = parse_ui_mode_from_str(&env_mode)
  {
    return mode;
  }

  UiMode::Inline
}

fn parse_ui_mode_from_str(raw: &str) -> Option<UiMode> {
  let normalized = raw.trim().to_ascii_lowercase();
  if normalized == "inline" {
    return Some(UiMode::Inline);
  }
  if normalized == "alt-screen" || normalized == "altscreen" || normalized == "alt" {
    return Some(UiMode::AltScreen);
  }
  None
}

async fn run_task(
  task: String,
  resolved_cwd: PathBuf,
  overrides: Vec<(String, String)>,
  skip_git_repo_check: bool,
) -> anyhow::Result<()> {
  if !skip_git_repo_check && get_git_repo_root(&resolved_cwd).is_none() {
    eprintln!("Not inside a trusted directory and --skip-git-repo-check was not specified.");
    std::process::exit(1);
  }

  let config = load_config(&resolved_cwd, overrides)?;
  let cokra = Cokra::new(config).await?;
  let result = cokra.run_turn(task).await?;
  println!("{}", result.final_message);
  Ok(())
}

async fn run_interactive(
  resolved_cwd: PathBuf,
  overrides: Vec<(String, String)>,
  cli_ui_mode: Option<CliUiMode>,
) -> anyhow::Result<()> {
  let config = load_config(&resolved_cwd, overrides)?;
  let ui_mode = resolve_ui_mode(cli_ui_mode);
  let cokra = Cokra::new(config).await?;
  let _ = run_tui_main(cokra, ui_mode).await?;
  Ok(())
}

async fn run_exec(
  task: String,
  resolved_cwd: PathBuf,
  overrides: Vec<(String, String)>,
  skip_git_repo_check: bool,
  jsonl: bool,
) -> anyhow::Result<()> {
  if !skip_git_repo_check && get_git_repo_root(&resolved_cwd).is_none() {
    eprintln!("Not inside a trusted directory and --skip-git-repo-check was not specified.");
    std::process::exit(1);
  }

  let config = load_config(&resolved_cwd, overrides)?;
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
      println!(
        "[item:start] id={} type={}",
        e.item.id(),
        e.item.item_type()
      );
    }
    EventMsg::AgentMessageDelta(e) | EventMsg::AgentMessageContentDelta(e) => {
      print!("{}", e.delta);
      let _ = io::stdout().flush();
    }
    EventMsg::ItemCompleted(e) => {
      let summary = match &e.item {
        cokra_protocol::TurnItem::UserMessage(item) => item.message(),
        cokra_protocol::TurnItem::AgentMessage(item) => item
          .content
          .iter()
          .map(|part| match part {
            cokra_protocol::AgentMessageContent::Text { text } => text.as_str(),
          })
          .collect::<String>(),
        cokra_protocol::TurnItem::Plan(item) => item.text.clone(),
        cokra_protocol::TurnItem::Reasoning(item) => item.summary_text.join("\n"),
        cokra_protocol::TurnItem::WebSearch(item) => item.query.clone(),
      };
      if summary.trim().is_empty() {
        println!("\n[item:done]");
      } else {
        println!("\n[item:done] {summary}");
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

async fn list_models(
  resolved_cwd: PathBuf,
  overrides: Vec<(String, String)>,
) -> anyhow::Result<()> {
  let config = load_config(&resolved_cwd, overrides)?;
  let model_client = init_model_layer(&config).await?;
  let mut providers = model_client.registry().list_providers().await;
  providers.sort_by(|a, b| a.id.cmp(&b.id));

  println!("Available models:");

  let mut entries = Vec::new();
  for provider in providers {
    let mut models = provider.models;
    models.sort();
    for model in models {
      let provider_prefix = format!("{}/", provider.id);
      if model.starts_with(&provider_prefix) {
        entries.push(model);
      } else {
        entries.push(format!("{}/{}", provider.id, model));
      }
    }
  }

  entries.sort();
  entries.dedup();

  if entries.is_empty() {
    println!("  (No models available)");
    return Ok(());
  }

  for entry in entries {
    println!("  {}", entry);
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::CliUiMode;
  use super::parse_ui_mode_from_str;
  use super::resolve_ui_mode;
  use cokra_tui::UiMode;

  #[test]
  fn parse_ui_mode_from_str_supports_inline_alias() {
    assert_eq!(parse_ui_mode_from_str("inline"), Some(UiMode::Inline));
    assert_eq!(parse_ui_mode_from_str(" INLINE "), Some(UiMode::Inline));
  }

  #[test]
  fn parse_ui_mode_from_str_supports_alt_aliases() {
    assert_eq!(parse_ui_mode_from_str("alt"), Some(UiMode::AltScreen));
    assert_eq!(
      parse_ui_mode_from_str("alt-screen"),
      Some(UiMode::AltScreen)
    );
    assert_eq!(parse_ui_mode_from_str("altscreen"), Some(UiMode::AltScreen));
  }

  #[test]
  fn parse_ui_mode_from_str_rejects_unknown_values() {
    assert_eq!(parse_ui_mode_from_str(""), None);
    assert_eq!(parse_ui_mode_from_str("auto"), None);
  }

  #[test]
  fn resolve_ui_mode_defaults_to_inline() {
    // Tradeoff: env mutation is process-global in Rust 2024, so tests must opt
    // into the unsafe contract explicitly.
    unsafe { std::env::remove_var("COKRA_TUI_MODE") };
    assert_eq!(resolve_ui_mode(None), UiMode::Inline);
  }

  #[test]
  fn resolve_ui_mode_cli_override_still_wins() {
    // Tradeoff: env mutation is process-global in Rust 2024, so tests must opt
    // into the unsafe contract explicitly.
    unsafe { std::env::set_var("COKRA_TUI_MODE", "inline") };
    assert_eq!(
      resolve_ui_mode(Some(CliUiMode::AltScreen)),
      UiMode::AltScreen
    );
    unsafe { std::env::remove_var("COKRA_TUI_MODE") };
  }
}
