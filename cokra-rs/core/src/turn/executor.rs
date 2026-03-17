//! Turn Executor
//!
//! Executes a turn (one user interaction cycle) in a Cokra session.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinError;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::team_runtime::runtime_for_thread;
use crate::compaction::CompactionSettings;
use crate::model::Message as ModelMessage;
use crate::model::ModelClient;
use crate::model::transform::ProviderRuntimeKind;
use crate::session::Session;
use crate::skills::injection::build_explicit_prompt_injections;
use crate::skills::injection::render_explicit_prompt_injections;
use crate::tool_runtime::ToolCapabilityFacets;
use crate::tool_runtime::ToolDefinition;
use crate::tool_runtime::ToolSource;
use crate::tool_runtime::UnifiedToolRuntime;
use crate::tools::registry::ToolRegistry;
use crate::tools::router::ToolRouter;
use crate::tools::spec::ToolSourceKind;
use crate::truncate::DEFAULT_TOOL_OUTPUT_TOKENS;
use crate::truncate::TruncationPolicy;
use cokra_protocol::AskForApproval;
use cokra_protocol::CompletionStatus;
use cokra_protocol::ErrorEvent;
use cokra_protocol::EventMsg;
use cokra_protocol::ModeKind;
use cokra_protocol::ReadOnlyAccess;
use cokra_protocol::SandboxPolicy;
use cokra_protocol::TurnCompleteEvent;
use cokra_protocol::TurnStartedEvent;

use super::response_items::ResponseItem;
use super::sse_executor::SseTurnExecutor;

type Event = cokra_protocol::EventMsg;

#[derive(Debug, Clone)]
struct PromptAssembly {
  prefix_messages: Vec<ModelMessage>,
  messages: Vec<ModelMessage>,
}

/// Turn executor errors
#[derive(Debug, thiserror::Error)]
pub enum TurnError {
  #[error("Model error: {0}")]
  ModelError(#[from] crate::model::ModelError),

  #[error("Tool error: {0}")]
  ToolError(String),

  #[error("Tool not found: {0}")]
  ToolNotFound(String),

  #[error("Session error: {0}")]
  SessionError(String),

  #[error("Context window exceeded")]
  ContextWindowExceeded,

  #[error("Turn aborted")]
  TurnAborted,

  #[error("Stream error: {0}")]
  Stream(String, Option<Duration>),

  #[error("Fatal error: {0}")]
  Fatal(String),
}

impl TurnError {
  pub fn is_retryable(&self) -> bool {
    matches!(self, TurnError::Stream(_, _))
  }
}

/// Turn execution result
#[derive(Debug, Clone)]
pub struct TurnResult {
  /// Final assistant text.
  pub content: String,
  /// Token usage summary.
  pub usage: crate::model::Usage,
  /// Whether the run completed successfully.
  pub success: bool,
}

/// Turn configuration
#[derive(Debug, Clone)]
pub struct TurnConfig {
  pub model: String,
  pub temperature: Option<f32>,
  pub max_tokens: Option<u32>,
  pub system_prompt: Option<String>,
  pub enable_tools: bool,
  pub approval_policy: AskForApproval,
  pub sandbox_policy: SandboxPolicy,
  pub cwd: PathBuf,
  pub has_managed_network_requirements: bool,
  pub allowed_domains: Vec<String>,
  pub denied_domains: Vec<String>,
  pub tool_output_truncation: TruncationPolicy,
  pub context_window_limit: Option<usize>,
  pub compaction: CompactionSettings,
}

impl Default for TurnConfig {
  fn default() -> Self {
    Self {
      model: "gpt-4o".to_string(),
      temperature: Some(0.2),
      max_tokens: Some(4096),
      system_prompt: None,
      enable_tools: true,
      approval_policy: AskForApproval::OnRequest,
      sandbox_policy: SandboxPolicy::ReadOnly {
        access: ReadOnlyAccess::FullAccess,
      },
      cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
      has_managed_network_requirements: false,
      allowed_domains: Vec::new(),
      denied_domains: Vec::new(),
      tool_output_truncation: TruncationPolicy::Tokens(DEFAULT_TOOL_OUTPUT_TOKENS),
      context_window_limit: None,
      compaction: CompactionSettings::default(),
    }
  }
}

#[derive(Clone)]
pub struct TurnExecutor {
  model_client: Arc<ModelClient>,
  tool_registry: Arc<ToolRegistry>,
  tool_router: Arc<ToolRouter>,
  tool_runtime: Option<Arc<UnifiedToolRuntime>>,
  session: Arc<Session>,
  tx_event: mpsc::Sender<Event>,
  config: TurnConfig,
  cancellation_token: CancellationToken,
}

impl TurnExecutor {
  pub fn new(
    model_client: Arc<ModelClient>,
    tool_registry: Arc<ToolRegistry>,
    tool_router: Arc<ToolRouter>,
    session: Arc<Session>,
    tx_event: mpsc::Sender<Event>,
    config: TurnConfig,
  ) -> Self {
    Self {
      model_client,
      tool_registry,
      tool_router,
      tool_runtime: None,
      session,
      tx_event,
      config,
      cancellation_token: CancellationToken::new(),
    }
  }

  pub fn with_tool_runtime(mut self, tool_runtime: Arc<UnifiedToolRuntime>) -> Self {
    self.tool_runtime = Some(tool_runtime);
    self
  }

  pub async fn run_turn(&self, input: UserInput) -> Result<TurnResult, TurnError> {
    self
      .run_turn_with_id(input, Uuid::new_v4().to_string())
      .await
  }

  pub async fn run_turn_with_id(
    &self,
    input: UserInput,
    turn_id: String,
  ) -> Result<TurnResult, TurnError> {
    let thread_id = self
      .session
      .thread_id()
      .cloned()
      .unwrap_or_default()
      .to_string();
    self.session.begin_turn(turn_id.clone()).await;

    let result = async {
      self
        .send_event(EventMsg::TurnStarted(TurnStartedEvent {
          thread_id: thread_id.clone(),
          turn_id: turn_id.clone(),
          mode: ModeKind::Default,
          model: self.config.model.clone(),
          cwd: self.config.cwd.clone(),
          start_time: chrono::Utc::now().timestamp(),
        }))
        .await?;

      let prompt = self.build_messages(&input).await?;
      let messages = prompt.messages.clone();

      let user_message = ModelMessage::User(input.content.clone());
      self.session.append_message(user_message.clone()).await;
      if let Some(item) = ResponseItem::from_model_message(&user_message) {
        self.session.append_response_item(item).await;
      }

      let sse_executor = SseTurnExecutor::new_with_cancellation(
        self.model_client.clone(),
        self.tool_registry.clone(),
        self.tool_router.clone(),
        self.session.clone(),
        self.tx_event.clone(),
        self.config.clone(),
        self.cancellation_token.child_token(),
      )
      .with_prompt_prefix(prompt.prefix_messages);

      let output = match sse_executor
        .run_sse_interaction(messages, thread_id.clone(), turn_id.clone())
        .await
      {
        Ok(output) => output,
        Err(TurnError::TurnAborted) => {
          self
            .send_event(EventMsg::TurnAborted(cokra_protocol::TurnAbortedEvent {
              thread_id: thread_id.clone(),
              turn_id: turn_id.clone(),
              reason: "turn aborted".to_string(),
            }))
            .await?;
          return Err(TurnError::TurnAborted);
        }
        Err(e) => {
          let error = e.to_string();
          let details = format!("{e:?}");
          self
            .send_event(EventMsg::Error(ErrorEvent {
              thread_id: thread_id.clone(),
              turn_id: turn_id.clone(),
              error: error.clone(),
              user_facing_message: error.clone(),
              details: details.clone(),
            }))
            .await?;
          self
            .send_event(EventMsg::TurnComplete(TurnCompleteEvent {
              thread_id: thread_id.clone(),
              turn_id: turn_id.clone(),
              status: CompletionStatus::Errored {
                error,
                user_facing_message: e.to_string(),
                details,
              },
              end_time: chrono::Utc::now().timestamp(),
            }))
            .await?;
          return Err(e);
        }
      };

      self
        .send_event(EventMsg::TurnComplete(TurnCompleteEvent {
          thread_id: thread_id.clone(),
          turn_id: turn_id.clone(),
          status: CompletionStatus::Success,
          end_time: chrono::Utc::now().timestamp(),
        }))
        .await?;

      Ok(output)
    }
    .await;

    self.session.end_turn(&turn_id).await;
    result
  }

  async fn build_messages(&self, input: &UserInput) -> Result<PromptAssembly, TurnError> {
    let mut prefix_messages = Vec::new();
    if let Some(system) = &self.config.system_prompt {
      prefix_messages.push(ModelMessage::System(system.clone()));
    }

    let env_context = format!(
      "<environment_context>\n  <cwd>{}</cwd>\n</environment_context>",
      self.config.cwd.display()
    );
    prefix_messages.push(ModelMessage::User(env_context));

    if let Some(thread_id) = self.session.thread_id().map(ToString::to_string)
      && let Some(team_runtime) = runtime_for_thread(&thread_id)
      && team_runtime.is_root_thread(&thread_id)
    {
      let snapshot = team_runtime.snapshot();
      if snapshot.members.len() > 1 {
        prefix_messages.push(ModelMessage::User(team_digest_prompt(&snapshot)));
      }
    }

    if let Some(summary) = self.build_runtime_tool_summary().await? {
      prefix_messages.push(ModelMessage::User(summary));
    }

    if let Some(ctx) = self.build_auto_context(&input.content).await? {
      prefix_messages.push(ModelMessage::User(ctx));
    }

    let explicit_injections =
      build_explicit_prompt_injections(&self.config.cwd, &input.content).await;
    if let Some(rendered) = render_explicit_prompt_injections(&explicit_injections) {
      prefix_messages.push(ModelMessage::User(rendered));
    }

    let history = if let Some(limit) = self.config.context_window_limit {
      self.session.get_history_for_prompt(limit).await
    } else {
      self.session.get_history(100).await
    };
    let mut messages = prefix_messages.clone();
    messages.extend(history);
    messages.push(ModelMessage::User(input.content.clone()));

    Ok(PromptAssembly {
      prefix_messages,
      messages,
    })
  }

  async fn build_auto_context(&self, content: &str) -> Result<Option<String>, TurnError> {
    let query = content.trim();
    if query.len() < 12 {
      return Ok(None);
    }

    let root = self.config.cwd.clone();
    let query_string = query.to_string();
    let params = cokra_file_search::SearchParams {
      root,
      query: query_string,
      max_scanned_files: 800,
      max_hits: 6,
      max_matches_per_file: 4,
      max_file_bytes: 192 * 1024,
    };

    let output = tokio::task::spawn_blocking(move || cokra_file_search::search(params))
      .await
      .map_err(map_auto_context_join_error)?
      .map_err(|err| TurnError::SessionError(format!("auto context search failed: {err:#}")))?;

    if output.hits.is_empty() {
      return Ok(None);
    }

    let mut rendered = String::new();
    rendered.push_str("<auto_context>\n");
    rendered.push_str("The following snippets were automatically selected from the workspace based on the user's request.\n");
    rendered.push_str("Use these paths and line numbers for navigation, and prefer read_file for full context when editing.\n\n");
    rendered.push_str(&format!("query: {}\n", output.query));
    rendered.push_str(&format!("root: {}\n", output.root.display()));
    rendered.push_str(&format!("truncated: {}\n\n", output.truncated));

    for hit in output.hits {
      rendered.push_str(&format!(
        "- file: {} (score: {})\n",
        hit.path.display(),
        hit.score
      ));
      for m in hit.matches {
        rendered.push_str(&format!("  - L{}: {}\n", m.line, m.text));
      }
      rendered.push('\n');
    }
    rendered.push_str("</auto_context>");

    Ok(Some(rendered))
  }

  async fn send_event(&self, event: Event) -> Result<(), TurnError> {
    self.session.emit_event(event.clone());
    self
      .tx_event
      .send(event)
      .await
      .map_err(|e| TurnError::SessionError(format!("failed to send event: {e}")))
  }

  pub fn cancel_current_turn(&self) {
    self.cancellation_token.cancel();
  }

  async fn build_runtime_tool_summary(&self) -> Result<Option<String>, TurnError> {
    let runtime_info = self
      .model_client
      .runtime_info_for_model(&self.config.model)
      .await
      .map_err(TurnError::ModelError)?;
    let lsp_status = crate::lsp::manager().status().await;
    if let Some(runtime) = &self.tool_runtime {
      let definitions = runtime.catalog().definitions();
      return Ok(render_runtime_tool_summary(
        definitions
          .into_iter()
          .filter(|tool| tool.enabled)
          .collect(),
        self.tool_registry.as_ref(),
        &self.config,
        &runtime_info.provider_id,
        runtime_info.runtime_kind,
        &lsp_status,
      ));
    }

    let fallback = self
      .tool_registry
      .active_specs()
      .into_iter()
      .map(|spec| ToolDefinition {
        id: spec.name.clone(),
        name: spec.name.clone(),
        description: spec.description.clone(),
        input_schema: spec.input_schema.to_value(),
        output_schema: spec.output_schema.as_ref().map(|schema| schema.to_value()),
        source: match spec.source_kind {
          ToolSourceKind::BuiltinPrimitive
          | ToolSourceKind::BuiltinCollaboration
          | ToolSourceKind::BuiltinWorkflow => ToolSource::Builtin,
          ToolSourceKind::Mcp => ToolSource::Mcp,
          ToolSourceKind::Cli => ToolSource::Cli,
          ToolSourceKind::Api => ToolSource::Api,
        },
        aliases: self.tool_registry.aliases_for(&spec.name),
        tags: Vec::new(),
        approval: crate::tool_runtime::ToolApproval::from_permissions(
          &spec.permissions,
          spec.permission_key.clone(),
          spec.mutates_state,
        ),
        enabled: true,
        supports_parallel: spec.supports_parallel,
        mutates_state: spec.mutates_state,
        input_keys: match &spec.input_schema {
          crate::tools::spec::JsonSchema::Object { properties, .. } => {
            properties.keys().cloned().collect()
          }
          _ => Vec::new(),
        },
        capabilities: ToolCapabilityFacets::for_tool_name(
          &spec.name,
          spec.permissions.allow_network,
        ),
        provider_id: None,
        source_kind: Some(
          match spec.source_kind {
            ToolSourceKind::BuiltinPrimitive => "builtin_primitive",
            ToolSourceKind::BuiltinCollaboration => "builtin_collaboration",
            ToolSourceKind::BuiltinWorkflow => "builtin_workflow",
            ToolSourceKind::Mcp => "mcp",
            ToolSourceKind::Cli => "cli",
            ToolSourceKind::Api => "api",
          }
          .to_string(),
        ),
        server_name: None,
        remote_name: None,
      })
      .collect::<Vec<_>>();

    Ok(render_runtime_tool_summary(
      fallback,
      self.tool_registry.as_ref(),
      &self.config,
      &runtime_info.provider_id,
      runtime_info.runtime_kind,
      &lsp_status,
    ))
  }
}

fn render_runtime_tool_summary(
  mut tools: Vec<ToolDefinition>,
  registry: &ToolRegistry,
  config: &TurnConfig,
  provider_id: &str,
  runtime_kind: ProviderRuntimeKind,
  lsp_status: &crate::lsp::LspManagerStatus,
) -> Option<String> {
  if tools.is_empty() {
    return None;
  }

  tools.sort_by(|left, right| left.name.cmp(&right.name));
  let active_tools = tools
    .iter()
    .filter(|tool| {
      tool.enabled
        && registry
          .get_spec(&tool.name)
          .is_none_or(|_| registry.is_active(&tool.name))
    })
    .cloned()
    .collect::<Vec<_>>();

  let source_order = [
    ToolSource::Builtin,
    ToolSource::Mcp,
    ToolSource::Cli,
    ToolSource::Api,
  ];
  let source_name = |source: ToolSource| match source {
    ToolSource::Builtin => "builtin",
    ToolSource::Mcp => "mcp",
    ToolSource::Cli => "cli",
    ToolSource::Api => "api",
  };

  let mut rendered = String::from(
    "<runtime_tool_summary>\n\
Use this block as the first source of truth for what tools are active in this session.\n\
When the user asks about the current tool space, available tools, or connected integrations:\n\
- use `search_tool` first\n\
- use `inspect_tool` when the user names a specific tool\n\
- use `active_tool_status` when you need a grouped active/inactive runtime summary\n\
- use `integration_status`, `connect_integration`, and `install_integration` for integration lifecycle work\n\
- do not start with repo search or project docs unless the user asks about implementation details\n",
  );

  rendered.push_str("Active tool counts by source:\n");
  for source in source_order {
    let count = active_tools
      .iter()
      .filter(|tool| tool.source == source)
      .count();
    if count > 0 {
      rendered.push_str(&format!("- {}: {}\n", source_name(source), count));
    }
  }

  let inactive_external = registry.inactive_external_tool_names();
  if !inactive_external.is_empty() {
    rendered.push_str(&format!(
      "Inactive external tools: {} (activate them before direct use when needed)\n",
      inactive_external.len()
    ));
  }

  rendered.push_str("Sample active tools by source:\n");
  for source in source_order {
    let names = active_tools
      .iter()
      .filter(|tool| tool.source == source)
      .map(|tool| tool.name.as_str())
      .take(8)
      .collect::<Vec<_>>();
    if !names.is_empty() {
      rendered.push_str(&format!(
        "- {}: {}\n",
        source_name(source),
        names.join(", ")
      ));
    }
  }

  let mut integration_sources = source_order
    .into_iter()
    .filter(|source| *source != ToolSource::Builtin)
    .filter_map(|source| {
      let mut providers = active_tools
        .iter()
        .filter(|tool| tool.source == source)
        .filter_map(|tool| tool.provider_id.clone())
        .collect::<Vec<_>>();
      providers.sort();
      providers.dedup();
      (!providers.is_empty()).then(|| {
        format!(
          "- {} providers: {}\n",
          source_name(source),
          providers.join(", ")
        )
      })
    })
    .collect::<Vec<_>>();

  if !integration_sources.is_empty() {
    rendered.push_str("Current integration sources:\n");
    for line in integration_sources.drain(..) {
      rendered.push_str(&line);
    }
  }

  rendered.push_str("Model runtime:\n");
  rendered.push_str(&format!("- provider: {provider_id}\n"));
  rendered.push_str(&format!(
    "- runtime_kind: {}\n",
    match runtime_kind {
      ProviderRuntimeKind::Standard => "standard",
      ProviderRuntimeKind::OpenAICodex => "openai_codex",
      ProviderRuntimeKind::GitHubCopilot => "github_copilot",
    }
  ));
  rendered.push_str(&format!(
    "- provider_native_web_search: {}\n",
    matches!(runtime_kind, ProviderRuntimeKind::OpenAICodex)
  ));

  let mut network_capabilities = active_tools
    .iter()
    .flat_map(|tool| tool.capabilities.network_backends.iter().cloned())
    .collect::<Vec<_>>();
  if matches!(runtime_kind, ProviderRuntimeKind::OpenAICodex) {
    network_capabilities.push("provider_native_openai_codex".to_string());
  }
  network_capabilities.sort();
  network_capabilities.dedup();
  if !network_capabilities.is_empty() {
    rendered.push_str("Network backends:\n");
    rendered.push_str(&format!(
      "- available: {}\n",
      network_capabilities.join(", ")
    ));
  }

  let interactive_exec_supported = active_tools
    .iter()
    .any(|tool| tool.capabilities.interactive_exec);
  let exec_tools = active_tools
    .iter()
    .filter(|tool| matches!(tool.name.as_str(), "shell" | "unified_exec"))
    .map(|tool| tool.name.as_str())
    .collect::<Vec<_>>();
  rendered.push_str("Command execution:\n");
  rendered.push_str(&format!(
    "- interactive_exec_supported: {}\n",
    interactive_exec_supported
  ));
  if !exec_tools.is_empty() {
    rendered.push_str(&format!("- available_tools: {}\n", exec_tools.join(", ")));
  }

  rendered.push_str("Code navigation policy:\n");
  rendered.push_str("- prefer `lsp` for definitions, references, hover, symbols, implementations, and call hierarchy\n");
  rendered.push_str("- use `code_search` for semantic workspace discovery or external code/doc context when LSP is unavailable\n");
  rendered.push_str(
    "- use `grep_files` for exact text/pattern scans when you already know the string to match\n",
  );
  rendered.push_str("- use `web_search` for current external information; provider-native web search may replace the local fallback when available\n");

  rendered.push_str("LSP service:\n");
  rendered.push_str(&format!("- enabled: {}\n", lsp_status.enabled));
  rendered.push_str(&format!("- auto_install: {}\n", lsp_status.auto_install));
  rendered.push_str(&format!(
    "- connected_clients: {}\n",
    lsp_status
      .clients
      .iter()
      .filter(|client| client.status == "connected")
      .count()
  ));
  rendered.push_str(&format!(
    "- broken_clients: {}\n",
    lsp_status
      .clients
      .iter()
      .filter(|client| client.status == "broken")
      .count()
  ));

  if config.has_managed_network_requirements
    || !config.allowed_domains.is_empty()
    || !config.denied_domains.is_empty()
  {
    rendered.push_str("Network policy:\n");
    rendered.push_str(&format!(
      "- managed_network_requirements: {}\n",
      config.has_managed_network_requirements
    ));
    if !config.allowed_domains.is_empty() {
      rendered.push_str(&format!(
        "- allowed_domains: {}\n",
        config.allowed_domains.join(", ")
      ));
    }
    if !config.denied_domains.is_empty() {
      rendered.push_str(&format!(
        "- denied_domains: {}\n",
        config.denied_domains.join(", ")
      ));
    }
  }

  rendered.push_str("</runtime_tool_summary>");
  Some(rendered)
}

fn map_auto_context_join_error(err: JoinError) -> TurnError {
  TurnError::SessionError(format!("auto context worker failed: {err}"))
}

#[derive(Debug, Clone)]
pub struct UserInput {
  pub content: String,
  pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone)]
pub struct Attachment {
  pub kind: AttachmentKind,
  pub data: Vec<u8>,
  pub mime_type: String,
}

#[derive(Debug, Clone)]
pub enum AttachmentKind {
  Image,
  File,
  PDF,
  Audio,
}

fn team_digest_prompt(snapshot: &cokra_protocol::TeamSnapshot) -> String {
  const MAX_TEAMMATES: usize = 6;
  const MAX_TASKS: usize = 3;
  const MAX_LOCKS: usize = 3;
  const MAX_TEXT_CHARS: usize = 120;

  fn truncate_chars(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
      return trimmed.to_string();
    }
    let mut out = trimmed
      .chars()
      .take(max_chars.saturating_sub(1))
      .collect::<String>();
    out.push('…');
    out
  }

  fn short_thread_id(thread_id: &str) -> String {
    let short = thread_id.chars().take(8).collect::<String>();
    if short.is_empty() {
      "agent".to_string()
    } else {
      short
    }
  }

  fn is_task_closed(task: &cokra_protocol::TeamTask) -> bool {
    matches!(
      task.status,
      cokra_protocol::TeamTaskStatus::Completed
        | cokra_protocol::TeamTaskStatus::Failed
        | cokra_protocol::TeamTaskStatus::Canceled
    ) || matches!(
      task.ready_state,
      cokra_protocol::TeamTaskReadyState::Completed
        | cokra_protocol::TeamTaskReadyState::Failed
        | cokra_protocol::TeamTaskReadyState::Canceled
    )
  }

  fn is_task_active(task: &cokra_protocol::TeamTask) -> bool {
    matches!(
      task.status,
      cokra_protocol::TeamTaskStatus::InProgress | cokra_protocol::TeamTaskStatus::Review
    ) || matches!(
      task.ready_state,
      cokra_protocol::TeamTaskReadyState::Claimed
        | cokra_protocol::TeamTaskReadyState::Review
        | cokra_protocol::TeamTaskReadyState::Blocked
    )
  }

  fn task_activity_rank(task: &cokra_protocol::TeamTask) -> i32 {
    use cokra_protocol::TeamTaskReadyState;
    use cokra_protocol::TeamTaskStatus;

    match (&task.status, &task.ready_state) {
      (TeamTaskStatus::Review, _) | (_, TeamTaskReadyState::Review) => 60,
      (TeamTaskStatus::InProgress, TeamTaskReadyState::Claimed) => 50,
      (TeamTaskStatus::InProgress, _) => 45,
      (_, TeamTaskReadyState::Blocked) => 40,
      (TeamTaskStatus::Pending, TeamTaskReadyState::Ready) => 35,
      (TeamTaskStatus::Pending, _) => 30,
      _ if is_task_closed(task) => 10,
      _ => 0,
    }
  }

  fn access_label(access: &cokra_protocol::OwnershipAccessMode) -> &'static str {
    match access {
      cokra_protocol::OwnershipAccessMode::SharedRead => "shared-read",
      cokra_protocol::OwnershipAccessMode::ExclusiveWrite => "exclusive-write",
      cokra_protocol::OwnershipAccessMode::Review => "review",
    }
  }

  let teammate_count = snapshot
    .members
    .iter()
    .filter(|member| member.thread_id != snapshot.root_thread_id)
    .count();

  let mut active_tasks = 0usize;
  let mut backlog_tasks = 0usize;
  let mut closed_tasks = 0usize;
  for task in &snapshot.tasks {
    if is_task_closed(task) {
      closed_tasks += 1;
    } else if is_task_active(task) {
      active_tasks += 1;
    } else {
      backlog_tasks += 1;
    }
  }
  let unread_total = snapshot.unread_counts.values().copied().sum::<usize>();

  let label_for = |thread_id: &str| -> String {
    if thread_id == snapshot.root_thread_id {
      return "@main".to_string();
    }
    snapshot
      .members
      .iter()
      .find(|member| member.thread_id == thread_id)
      .and_then(|member| {
        member
          .nickname
          .as_deref()
          .map(str::trim)
          .filter(|it| !it.is_empty())
      })
      .map(|nickname| format!("@{nickname}"))
      .unwrap_or_else(|| format!("@{}", short_thread_id(thread_id)))
  };

  let mut out = String::new();
  use std::fmt::Write as _;

  let _ = writeln!(out, "<team_digest>");
  let _ = writeln!(
    out,
    "Team status: {teammate_count} teammate(s) | tasks: active {active_tasks}, backlog {backlog_tasks}, closed {closed_tasks} | locks {} | unread {unread_total}",
    snapshot.ownership_leases.len(),
  );

  if teammate_count > 0 {
    let _ = writeln!(out, "");
    let _ = writeln!(out, "Teammates:");

    let mut members = snapshot
      .members
      .iter()
      .filter(|member| member.thread_id != snapshot.root_thread_id)
      .collect::<Vec<_>>();
    members.sort_by(|left, right| {
      fn lifecycle_rank(lifecycle: &cokra_protocol::CollabAgentLifecycle) -> u8 {
        match lifecycle {
          cokra_protocol::CollabAgentLifecycle::Error => 0,
          cokra_protocol::CollabAgentLifecycle::Busy => 1,
          cokra_protocol::CollabAgentLifecycle::PendingInit => 2,
          cokra_protocol::CollabAgentLifecycle::Ready => 3,
          cokra_protocol::CollabAgentLifecycle::Shutdown => 4,
          cokra_protocol::CollabAgentLifecycle::NotFound => 5,
        }
      }

      lifecycle_rank(&left.state.lifecycle)
        .cmp(&lifecycle_rank(&right.state.lifecycle))
        .then_with(|| {
          right
            .state
            .pending_wake_count
            .cmp(&left.state.pending_wake_count)
        })
        .then_with(|| left.thread_id.cmp(&right.thread_id))
    });

    for member in members.into_iter().take(MAX_TEAMMATES) {
      let nickname = member
        .nickname
        .as_deref()
        .map(str::trim)
        .filter(|it| !it.is_empty());
      let role = member.role.trim();
      let mut label = nickname
        .map(|nickname| format!("@{nickname}"))
        .unwrap_or_else(|| format!("@{}", short_thread_id(&member.thread_id)));
      if !role.is_empty() && !role.eq_ignore_ascii_case("default") {
        label.push_str(&format!(" [{role}]"));
      }

      let lifecycle = match member.state.lifecycle.clone() {
        cokra_protocol::CollabAgentLifecycle::PendingInit => "PendingInit",
        cokra_protocol::CollabAgentLifecycle::Ready => "Ready",
        cokra_protocol::CollabAgentLifecycle::Busy => "Busy",
        cokra_protocol::CollabAgentLifecycle::Error => "Error",
        cokra_protocol::CollabAgentLifecycle::Shutdown => "Shutdown",
        cokra_protocol::CollabAgentLifecycle::NotFound => "NotFound",
      };

      let mut line = format!("- {label}: {lifecycle}");

      let unread = snapshot
        .unread_counts
        .get(&member.thread_id)
        .copied()
        .unwrap_or(0);
      if unread > 0 {
        line.push_str(&format!(" · {unread} unread"));
      }
      if member.state.pending_wake_count > 0 {
        line.push_str(&format!(" · {} queued", member.state.pending_wake_count));
      }

      if let Some(reason) = member
        .state
        .attention_reason
        .as_deref()
        .map(str::trim)
        .filter(|it| !it.is_empty())
      {
        line.push_str(&format!(
          " · attention: {}",
          truncate_chars(reason, MAX_TEXT_CHARS)
        ));
      } else {
        let maybe_task = snapshot
          .tasks
          .iter()
          .filter(|task| {
            task.owner_thread_id.as_deref() == Some(&member.thread_id)
              || task.assignee_thread_id.as_deref() == Some(&member.thread_id)
              || task.reviewer_thread_id.as_deref() == Some(&member.thread_id)
          })
          .max_by_key(|task| (task_activity_rank(task), task.updated_at));

        if let Some(task) = maybe_task.filter(|task| !is_task_closed(task)) {
          if matches!(
            task.ready_state,
            cokra_protocol::TeamTaskReadyState::Blocked
          ) {
            line.push_str(&format!(
              " · blocked: {}",
              truncate_chars(
                task.blocking_reason.as_deref().unwrap_or("blocked"),
                MAX_TEXT_CHARS
              )
            ));
          } else if matches!(task.status, cokra_protocol::TeamTaskStatus::Review)
            || matches!(task.ready_state, cokra_protocol::TeamTaskReadyState::Review)
            || matches!(
              task.review_state,
              cokra_protocol::TeamTaskReviewState::Requested
            )
          {
            line.push_str(&format!(
              " · reviewing #{id} {title}",
              id = task.id,
              title = truncate_chars(&task.title, 80)
            ));
          } else if matches!(task.status, cokra_protocol::TeamTaskStatus::InProgress)
            || matches!(
              task.ready_state,
              cokra_protocol::TeamTaskReadyState::Claimed
            )
          {
            line.push_str(&format!(
              " · working #{id} {title}",
              id = task.id,
              title = truncate_chars(&task.title, 80)
            ));
          } else if matches!(task.ready_state, cokra_protocol::TeamTaskReadyState::Ready) {
            line.push_str(&format!(
              " · ready #{id} {title}",
              id = task.id,
              title = truncate_chars(&task.title, 80)
            ));
          }
        }
      }

      let _ = writeln!(out, "{line}");
    }

    if snapshot.tasks.iter().any(|task| !is_task_closed(task)) {
      let _ = writeln!(out, "");
      let _ = writeln!(out, "Top tasks:");
      let mut tasks = snapshot
        .tasks
        .iter()
        .filter(|task| !is_task_closed(task))
        .collect::<Vec<_>>();
      tasks.sort_by(|left, right| {
        task_activity_rank(right)
          .cmp(&task_activity_rank(left))
          .then_with(|| right.updated_at.cmp(&left.updated_at))
          .then_with(|| left.id.cmp(&right.id))
      });

      for task in tasks.into_iter().take(MAX_TASKS) {
        let owner = task
          .owner_thread_id
          .as_deref()
          .map(label_for)
          .unwrap_or_else(|| "unassigned".to_string());
        let _ = writeln!(
          out,
          "- #{id} {status:?}/{ready_state:?} owner={owner} | {title}",
          id = task.id,
          status = task.status,
          ready_state = task.ready_state,
          title = truncate_chars(&task.title, 80),
        );
      }
    }

    if !snapshot.ownership_leases.is_empty() {
      let _ = writeln!(out, "");
      let _ = writeln!(out, "Active locks:");
      for lease in snapshot.ownership_leases.iter().take(MAX_LOCKS) {
        let owner = label_for(&lease.owner_thread_id);
        let _ = writeln!(
          out,
          "- {access} {kind:?} {path} by {owner} (task {task_id})",
          access = access_label(&lease.access),
          kind = lease.scope.kind,
          path = truncate_chars(&lease.scope.path, 80),
          task_id = lease.task_id
        );
      }
    }
  }

  let _ = writeln!(out, "");
  let _ = writeln!(
    out,
    "Guidance: Prefer waiting for assigned teammates; avoid mutating paths you haven't claimed in the task graph."
  );
  let _ = writeln!(out, "</team_digest>");
  out
}

#[cfg(test)]
mod tests {
  use std::collections::BTreeMap;
  use std::pin::Pin;
  use std::sync::Arc;

  use async_trait::async_trait;
  use futures::Stream;
  use reqwest::Client;
  use tempfile::tempdir;
  use tokio::sync::Mutex;
  use tokio::sync::mpsc;

  use cokra_protocol::ContentDeltaEvent;
  use cokra_protocol::EventMsg;
  use cokra_protocol::ResponseEvent;

  use super::TurnConfig;
  use super::TurnExecutor;
  use super::UserInput;
  use crate::model::ChatRequest;
  use crate::model::ChatResponse;
  use crate::model::Chunk;
  use crate::model::ListModelsResponse;
  use crate::model::ModelClient;
  use crate::model::ModelError;
  use crate::model::ModelInfo;
  use crate::model::ProviderConfig;
  use crate::model::ProviderRegistry;
  use crate::model::provider::ModelProvider;
  use crate::session::Session;
  use crate::tool_runtime::BuiltinToolProvider;
  use crate::tool_runtime::CliToolProvider;
  use crate::tool_runtime::ToolProvider;
  use crate::tool_runtime::ToolRuntimeCatalog;
  use crate::tool_runtime::ToolSource;
  use crate::tool_runtime::UnifiedToolRuntime;
  use crate::tools::registry::ToolRegistry;
  use crate::tools::router::ToolRouter;
  use crate::tools::spec::JsonSchema;
  use crate::tools::spec::ToolHandlerType;
  use crate::tools::spec::ToolPermissions;
  use crate::tools::spec::ToolSpec;
  use crate::tools::validation::ToolValidator;
  use cokra_config::ApprovalMode;
  use cokra_config::ApprovalPolicy;
  use cokra_config::PatchApproval;
  use cokra_config::SandboxConfig;
  use cokra_config::SandboxMode;
  use cokra_config::ShellApproval;

  #[derive(Debug)]
  struct OrderedProvider {
    client: Client,
    config: ProviderConfig,
    scripts: Arc<Mutex<Vec<Vec<ResponseEvent>>>>,
  }

  impl OrderedProvider {
    fn new(scripts: Vec<Vec<ResponseEvent>>) -> Self {
      Self {
        client: Client::new(),
        config: ProviderConfig {
          provider_id: "mock-order".to_string(),
          ..Default::default()
        },
        scripts: Arc::new(Mutex::new(scripts)),
      }
    }
  }

  #[async_trait]
  impl ModelProvider for OrderedProvider {
    fn provider_id(&self) -> &'static str {
      "mock-order"
    }

    fn provider_name(&self) -> &'static str {
      "Mock Ordered Provider"
    }

    async fn chat_completion(&self, _request: ChatRequest) -> crate::model::Result<ChatResponse> {
      Err(ModelError::InvalidRequest(
        "chat_completion is unused in this test provider".to_string(),
      ))
    }

    async fn chat_completion_stream(
      &self,
      _request: ChatRequest,
    ) -> crate::model::Result<Pin<Box<dyn Stream<Item = crate::model::Result<Chunk>> + Send>>> {
      Ok(Box::pin(futures::stream::empty()))
    }

    async fn responses_stream(
      &self,
      _request: ChatRequest,
    ) -> crate::model::Result<Pin<Box<dyn Stream<Item = crate::model::Result<ResponseEvent>> + Send>>>
    {
      let mut scripts = self.scripts.lock().await;
      if scripts.is_empty() {
        return Err(ModelError::InvalidRequest(
          "mock response script exhausted".to_string(),
        ));
      }
      let script = scripts.remove(0);
      Ok(Box::pin(futures::stream::iter(
        script.into_iter().map(Ok::<ResponseEvent, ModelError>),
      )))
    }

    async fn list_models(&self) -> crate::model::Result<ListModelsResponse> {
      Ok(ListModelsResponse {
        object_type: "list".to_string(),
        data: vec![ModelInfo {
          id: "mock-order/model".to_string(),
          object_type: "model".to_string(),
          created: 0,
          owned_by: Some("mock".to_string()),
        }],
      })
    }

    async fn validate_auth(&self) -> crate::model::Result<()> {
      Ok(())
    }

    fn client(&self) -> &Client {
      &self.client
    }

    fn config(&self) -> &ProviderConfig {
      &self.config
    }
  }

  async fn build_client(provider: OrderedProvider) -> Arc<ModelClient> {
    let registry = Arc::new(ProviderRegistry::new());
    registry.register(provider).await;
    registry
      .set_default("mock-order")
      .await
      .expect("set default provider");

    Arc::new(
      ModelClient::new(registry)
        .await
        .expect("create model client"),
    )
  }

  fn test_config() -> TurnConfig {
    TurnConfig {
      model: "mock-order/model".to_string(),
      temperature: None,
      max_tokens: None,
      system_prompt: None,
      enable_tools: false,
      ..TurnConfig::default()
    }
  }

  fn build_router(registry: Arc<ToolRegistry>) -> Arc<ToolRouter> {
    Arc::new(ToolRouter::new(
      registry,
      Arc::new(ToolValidator::new(
        SandboxConfig {
          mode: SandboxMode::Permissive,
          network_access: false,
        },
        ApprovalPolicy {
          policy: ApprovalMode::Auto,
          shell: ShellApproval::OnFailure,
          patch: PatchApproval::OnRequest,
        },
      )),
    ))
  }

  #[tokio::test]
  async fn test_sse_event_ordering() {
    let provider = OrderedProvider::new(vec![vec![
      ResponseEvent::ContentDelta(ContentDeltaEvent {
        text: "Hello".to_string(),
        index: 0,
      }),
      ResponseEvent::ContentDelta(ContentDeltaEvent {
        text: " world".to_string(),
        index: 1,
      }),
      ResponseEvent::EndTurn,
    ]]);

    let model_client = build_client(provider).await;
    let tool_registry = Arc::new(ToolRegistry::new());
    let tool_router = build_router(tool_registry.clone());
    let session = Arc::new(Session::new());
    let expected_thread_id = session.thread_id().cloned().unwrap_or_default().to_string();
    let (tx_event, mut rx_event) = mpsc::channel(64);

    let executor = TurnExecutor::new(
      model_client,
      tool_registry,
      tool_router,
      session,
      tx_event,
      test_config(),
    );

    let result = executor
      .run_turn(UserInput {
        content: "hello".to_string(),
        attachments: Vec::new(),
      })
      .await
      .expect("run turn");
    assert_eq!(result.content, "Hello world");

    let mut labels = Vec::new();
    let mut turn_started_thread_id = None;
    while let Ok(event) = rx_event.try_recv() {
      match event {
        EventMsg::TurnStarted(ev) => {
          turn_started_thread_id = Some(ev.thread_id.clone());
          labels.push("turn_started");
        }
        EventMsg::ItemStarted(_) => labels.push("item_started"),
        EventMsg::AgentMessageContentDelta(_) => labels.push("delta"),
        EventMsg::ItemCompleted(_) => labels.push("item_completed"),
        EventMsg::TurnComplete(_) => labels.push("turn_complete"),
        _ => {}
      }
    }

    assert_eq!(
      labels,
      vec![
        "turn_started",
        "item_started",
        "delta",
        "delta",
        "item_completed",
        "turn_complete",
      ]
    );
    assert_eq!(
      turn_started_thread_id.as_deref(),
      Some(expected_thread_id.as_str())
    );
  }

  #[tokio::test]
  async fn build_messages_includes_auto_context_snippets() -> anyhow::Result<()> {
    let temp = tempdir().expect("tempdir");
    let root = temp.path();
    std::fs::create_dir_all(root.join("core/src/tools")).expect("create dirs");
    let target = root.join("core/src/tools/registry.rs");
    std::fs::write(
      &target,
      "pub struct ToolRegistry {}\nimpl ToolRegistry { pub fn new() -> Self { Self {} } }\n",
    )
    .expect("write");

    let model_client = build_client(OrderedProvider::new(vec![vec![ResponseEvent::EndTurn]])).await;
    let tool_registry = Arc::new(ToolRegistry::new());
    let tool_router = build_router(tool_registry.clone());
    let session = Arc::new(Session::new());
    let (tx_event, _rx_event) = mpsc::channel(64);

    let mut cfg = test_config();
    cfg.cwd = root.to_path_buf();

    let executor = TurnExecutor::new(
      model_client,
      tool_registry,
      tool_router,
      session,
      tx_event,
      cfg,
    );

    let prompt = executor
      .build_messages(&UserInput {
        content: "Where is ToolRegistry registered?".to_string(),
        attachments: Vec::new(),
      })
      .await?;

    let ctx = prompt.messages.iter().filter_map(|msg| match msg {
      crate::model::Message::User(text) if text.contains("<auto_context>") => Some(text),
      _ => None,
    });

    let rendered = ctx.map(String::as_str).collect::<Vec<_>>().join("\n");
    assert!(rendered.contains("ToolRegistry"));
    assert!(rendered.contains(&target.display().to_string()));
    Ok(())
  }

  #[tokio::test]
  async fn build_messages_includes_explicit_skill_injections() -> anyhow::Result<()> {
    let temp = tempdir().expect("tempdir");
    let root = temp.path();
    let skill_dir = root.join(".cokra").join("skills").join("rust-expert");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(
      skill_dir.join("SKILL.md"),
      "---\nname: rust-expert\ndescription: Rust specialist\n---\n\nPrefer ownership-safe refactors.",
    )
    .expect("write skill");

    let model_client = build_client(OrderedProvider::new(vec![vec![ResponseEvent::EndTurn]])).await;
    let tool_registry = Arc::new(ToolRegistry::new());
    let tool_router = build_router(tool_registry.clone());
    let session = Arc::new(Session::new());
    let (tx_event, _rx_event) = mpsc::channel(64);

    let mut cfg = test_config();
    cfg.cwd = root.to_path_buf();

    let executor = TurnExecutor::new(
      model_client,
      tool_registry,
      tool_router,
      session,
      tx_event,
      cfg,
    );

    let prompt = executor
      .build_messages(&UserInput {
        content: "Use $rust-expert for this change.".to_string(),
        attachments: Vec::new(),
      })
      .await?;

    let rendered = prompt
      .messages
      .iter()
      .filter_map(|msg| match msg {
        crate::model::Message::User(text) if text.contains("<explicit_injections>") => Some(text),
        _ => None,
      })
      .map(String::as_str)
      .collect::<Vec<_>>()
      .join("\n");

    assert!(rendered.contains("rust-expert"));
    assert!(rendered.contains("Prefer ownership-safe refactors."));
    Ok(())
  }

  #[tokio::test]
  async fn build_messages_includes_runtime_tool_summary() -> anyhow::Result<()> {
    let model_client = build_client(OrderedProvider::new(vec![vec![ResponseEvent::EndTurn]])).await;
    let mut registry = ToolRegistry::new();
    registry.register_spec(ToolSpec::new(
      "search_tool",
      "Search the current runtime tool space.",
      JsonSchema::Object {
        properties: BTreeMap::from([(
          "query".to_string(),
          JsonSchema::String { description: None },
        )]),
        required: Some(vec!["query".to_string()]),
        additional_properties: Some(false.into()),
      },
      None,
      ToolHandlerType::Function,
      ToolPermissions::default(),
    ));
    registry.register_spec(ToolSpec::new(
      "inspect_tool",
      "Inspect a specific tool definition.",
      JsonSchema::Object {
        properties: BTreeMap::from([(
          "name".to_string(),
          JsonSchema::String { description: None },
        )]),
        required: Some(vec!["name".to_string()]),
        additional_properties: Some(false.into()),
      },
      None,
      ToolHandlerType::Function,
      ToolPermissions::default(),
    ));

    let tool_registry = Arc::new(registry);
    let tool_router = build_router(tool_registry.clone());
    let session = Arc::new(Session::new());
    let (tx_event, _rx_event) = mpsc::channel(64);

    let builtin_provider: Arc<dyn ToolProvider> =
      Arc::new(BuiltinToolProvider::from_registry(&tool_registry));
    let cli_provider: Arc<dyn ToolProvider> = Arc::new(CliToolProvider::new(
      "demo-cli",
      vec![crate::tool_runtime::ToolDefinition {
        id: "echo_demo".to_string(),
        name: "echo_demo".to_string(),
        description: "Echo text through a CLI integration".to_string(),
        input_schema: serde_json::json!({
          "type": "object",
          "properties": { "text": { "type": "string" } },
          "required": ["text"]
        }),
        output_schema: None,
        source: ToolSource::Cli,
        aliases: Vec::new(),
        tags: vec!["cli".to_string(), "demo-cli".to_string()],
        approval: crate::tool_runtime::ToolApproval {
          risk_level: crate::tool_runtime::ToolRiskLevel::Low,
          approval_mode: crate::tool_runtime::ApprovalMode::Auto,
          permission_key: Some("echo_demo".to_string()),
          allow_network: false,
          allow_fs_write: false,
        },
        enabled: true,
        supports_parallel: true,
        mutates_state: false,
        input_keys: vec!["text".to_string()],
        capabilities: crate::tool_runtime::ToolCapabilityFacets::for_tool_name("echo_demo", false),
        provider_id: Some("demo-cli".to_string()),
        source_kind: Some("cli".to_string()),
        server_name: None,
        remote_name: None,
      }],
    ));
    let catalog =
      Arc::new(ToolRuntimeCatalog::from_providers(&[builtin_provider, cli_provider]).await?);
    let runtime = Arc::new(UnifiedToolRuntime::new(
      catalog,
      Vec::new(),
      tool_router.clone(),
    ));

    let executor = TurnExecutor::new(
      model_client,
      tool_registry,
      tool_router,
      session,
      tx_event,
      test_config(),
    )
    .with_tool_runtime(runtime);

    let prompt = executor
      .build_messages(&UserInput {
        content: "What tools are available right now?".to_string(),
        attachments: Vec::new(),
      })
      .await?;

    let rendered = prompt
      .messages
      .iter()
      .filter_map(|msg| match msg {
        crate::model::Message::User(text) if text.contains("<runtime_tool_summary>") => Some(text),
        _ => None,
      })
      .map(String::as_str)
      .collect::<Vec<_>>()
      .join("\n");

    assert!(rendered.contains("search_tool"));
    assert!(rendered.contains("inspect_tool"));
    assert!(rendered.contains("echo_demo"));
    assert!(rendered.contains("demo-cli"));
    assert!(rendered.contains("use `search_tool` first"));
    Ok(())
  }

  #[test]
  fn runtime_tool_summary_snapshot() {
    let mut registry = ToolRegistry::new();
    registry.register_spec(ToolSpec::new(
      "search_tool",
      "Search the current runtime tool space.",
      JsonSchema::Object {
        properties: BTreeMap::new(),
        required: Some(Vec::new()),
        additional_properties: Some(false.into()),
      },
      None,
      ToolHandlerType::Function,
      ToolPermissions::default(),
    ));
    registry.register_spec(ToolSpec::new(
      "inspect_tool",
      "Inspect a specific tool definition.",
      JsonSchema::Object {
        properties: BTreeMap::new(),
        required: Some(Vec::new()),
        additional_properties: Some(false.into()),
      },
      None,
      ToolHandlerType::Function,
      ToolPermissions::default(),
    ));
    registry.register_spec(
      ToolSpec::new(
        "echo_demo",
        "CLI demo tool",
        JsonSchema::Object {
          properties: BTreeMap::new(),
          required: Some(Vec::new()),
          additional_properties: Some(false.into()),
        },
        None,
        ToolHandlerType::Function,
        ToolPermissions::default(),
      )
      .with_source_kind(crate::tools::spec::ToolSourceKind::Cli),
    );
    registry.register_spec(
      ToolSpec::new(
        "ping_api",
        "API demo tool",
        JsonSchema::Object {
          properties: BTreeMap::new(),
          required: Some(Vec::new()),
          additional_properties: Some(false.into()),
        },
        None,
        ToolHandlerType::Function,
        ToolPermissions::default(),
      )
      .with_source_kind(crate::tools::spec::ToolSourceKind::Api),
    );
    registry.deactivate_tool("echo_demo");

    let config = TurnConfig {
      has_managed_network_requirements: true,
      allowed_domains: vec!["docs.rs".to_string(), "api.openai.com".to_string()],
      denied_domains: vec!["example.com".to_string()],
      ..test_config()
    };

    let summary = super::render_runtime_tool_summary(
      vec![
        crate::tool_runtime::ToolDefinition {
          id: "search_tool".to_string(),
          name: "search_tool".to_string(),
          description: "Search the current runtime tool space.".to_string(),
          input_schema: serde_json::json!({"type": "object", "properties": {}}),
          output_schema: None,
          source: ToolSource::Builtin,
          aliases: Vec::new(),
          tags: Vec::new(),
          approval: crate::tool_runtime::ToolApproval {
            risk_level: crate::tool_runtime::ToolRiskLevel::Low,
            approval_mode: crate::tool_runtime::ApprovalMode::Auto,
            permission_key: Some("tool_catalog".to_string()),
            allow_network: false,
            allow_fs_write: false,
          },
          enabled: true,
          supports_parallel: true,
          mutates_state: false,
          input_keys: Vec::new(),
          capabilities: crate::tool_runtime::ToolCapabilityFacets::for_tool_name(
            "search_tool",
            false,
          ),
          provider_id: Some("builtin".to_string()),
          source_kind: Some("builtin_primitive".to_string()),
          server_name: None,
          remote_name: None,
        },
        crate::tool_runtime::ToolDefinition {
          id: "inspect_tool".to_string(),
          name: "inspect_tool".to_string(),
          description: "Inspect a specific tool definition.".to_string(),
          input_schema: serde_json::json!({"type": "object", "properties": {}}),
          output_schema: None,
          source: ToolSource::Builtin,
          aliases: Vec::new(),
          tags: Vec::new(),
          approval: crate::tool_runtime::ToolApproval {
            risk_level: crate::tool_runtime::ToolRiskLevel::Low,
            approval_mode: crate::tool_runtime::ApprovalMode::Auto,
            permission_key: Some("tool_catalog".to_string()),
            allow_network: false,
            allow_fs_write: false,
          },
          enabled: true,
          supports_parallel: true,
          mutates_state: false,
          input_keys: Vec::new(),
          capabilities: crate::tool_runtime::ToolCapabilityFacets::for_tool_name(
            "inspect_tool",
            false,
          ),
          provider_id: Some("builtin".to_string()),
          source_kind: Some("builtin_primitive".to_string()),
          server_name: None,
          remote_name: None,
        },
        crate::tool_runtime::ToolDefinition {
          id: "echo_demo".to_string(),
          name: "echo_demo".to_string(),
          description: "CLI demo tool".to_string(),
          input_schema: serde_json::json!({"type": "object", "properties": {}}),
          output_schema: None,
          source: ToolSource::Cli,
          aliases: Vec::new(),
          tags: Vec::new(),
          approval: crate::tool_runtime::ToolApproval {
            risk_level: crate::tool_runtime::ToolRiskLevel::Low,
            approval_mode: crate::tool_runtime::ApprovalMode::Auto,
            permission_key: Some("echo_demo".to_string()),
            allow_network: false,
            allow_fs_write: false,
          },
          enabled: true,
          supports_parallel: true,
          mutates_state: false,
          input_keys: Vec::new(),
          capabilities: crate::tool_runtime::ToolCapabilityFacets::for_tool_name(
            "echo_demo",
            false,
          ),
          provider_id: Some("demo-cli".to_string()),
          source_kind: Some("cli".to_string()),
          server_name: None,
          remote_name: None,
        },
        crate::tool_runtime::ToolDefinition {
          id: "ping_api".to_string(),
          name: "ping_api".to_string(),
          description: "API demo tool".to_string(),
          input_schema: serde_json::json!({"type": "object", "properties": {}}),
          output_schema: None,
          source: ToolSource::Api,
          aliases: Vec::new(),
          tags: Vec::new(),
          approval: crate::tool_runtime::ToolApproval {
            risk_level: crate::tool_runtime::ToolRiskLevel::Low,
            approval_mode: crate::tool_runtime::ApprovalMode::Auto,
            permission_key: Some("ping_api".to_string()),
            allow_network: false,
            allow_fs_write: false,
          },
          enabled: true,
          supports_parallel: true,
          mutates_state: false,
          input_keys: Vec::new(),
          capabilities: crate::tool_runtime::ToolCapabilityFacets::for_tool_name("ping_api", false),
          provider_id: Some("demo-api".to_string()),
          source_kind: Some("api".to_string()),
          server_name: None,
          remote_name: None,
        },
      ],
      &registry,
      &config,
      "openai",
      crate::model::transform::ProviderRuntimeKind::OpenAICodex,
      &crate::lsp::LspManagerStatus {
        enabled: true,
        auto_install: true,
        request_timeout_ms: 15_000,
        diagnostics_timeout_ms: 5_000,
        clients: Vec::new(),
      },
    )
    .expect("runtime summary");

    insta::assert_snapshot!(
      summary,
      @r###"
<runtime_tool_summary>
Use this block as the first source of truth for what tools are active in this session.
When the user asks about the current tool space, available tools, or connected integrations:
- use `search_tool` first
- use `inspect_tool` when the user names a specific tool
- use `active_tool_status` when you need a grouped active/inactive runtime summary
- use `integration_status`, `connect_integration`, and `install_integration` for integration lifecycle work
- do not start with repo search or project docs unless the user asks about implementation details
Active tool counts by source:
- builtin: 2
- api: 1
Inactive external tools: 1 (activate them before direct use when needed)
Sample active tools by source:
- builtin: inspect_tool, search_tool
- api: ping_api
Current integration sources:
- api providers: demo-api
Model runtime:
- provider: openai
- runtime_kind: openai_codex
- provider_native_web_search: true
Network backends:
- available: provider_native_openai_codex
Command execution:
- interactive_exec_supported: false
Code navigation policy:
- prefer `lsp` for definitions, references, hover, symbols, implementations, and call hierarchy
- use `code_search` for semantic workspace discovery or external code/doc context when LSP is unavailable
- use `grep_files` for exact text/pattern scans when you already know the string to match
- use `web_search` for current external information; provider-native web search may replace the local fallback when available
LSP service:
- enabled: true
- auto_install: true
- connected_clients: 0
- broken_clients: 0
Network policy:
- managed_network_requirements: true
- allowed_domains: docs.rs, api.openai.com
- denied_domains: example.com
</runtime_tool_summary>
"###
    );
  }

  #[tokio::test]
  async fn test_error_path_emits_terminal_turn_complete() {
    let provider = OrderedProvider::new(vec![vec![ResponseEvent::Error(
      cokra_protocol::ResponseErrorEvent {
        message: "boom".to_string(),
      },
    )]]);

    let model_client = build_client(provider).await;
    let tool_registry = Arc::new(ToolRegistry::new());
    let tool_router = build_router(tool_registry.clone());
    let session = Arc::new(Session::new());
    let (tx_event, mut rx_event) = mpsc::channel(64);

    let executor = TurnExecutor::new(
      model_client,
      tool_registry,
      tool_router,
      session,
      tx_event,
      test_config(),
    );

    let result = executor
      .run_turn(UserInput {
        content: "hello".to_string(),
        attachments: Vec::new(),
      })
      .await;
    assert!(result.is_err());

    let mut labels = Vec::new();
    let mut saw_error = false;
    let mut saw_errored_complete = false;
    while let Ok(event) = rx_event.try_recv() {
      match event {
        EventMsg::TurnStarted(_) => labels.push("turn_started"),
        EventMsg::ItemStarted(_) => labels.push("item_started"),
        EventMsg::Error(err) => {
          saw_error = err.user_facing_message.contains("boom");
          labels.push("error");
        }
        EventMsg::TurnComplete(done) => {
          saw_errored_complete = matches!(
            done.status,
            cokra_protocol::CompletionStatus::Errored { .. }
          );
          labels.push("turn_complete");
        }
        _ => {}
      }
    }

    assert!(saw_error);
    assert!(saw_errored_complete);
    assert_eq!(
      labels,
      vec!["turn_started", "item_started", "error", "turn_complete"]
    );
  }
}
