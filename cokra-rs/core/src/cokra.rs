use std::collections::VecDeque;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use futures::Stream;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::watch;
use uuid::Uuid;

use cokra_config::ApprovalMode;
use cokra_config::Config;
use cokra_config::SandboxMode;
use cokra_protocol::AskForApproval;
use cokra_protocol::CompletionStatus;
use cokra_protocol::Event;
use cokra_protocol::EventMsg;
use cokra_protocol::Op;
use cokra_protocol::ReadOnlyAccess;
use cokra_protocol::SandboxPolicy;
use cokra_protocol::SessionConfiguredEvent;
use cokra_protocol::Submission;
use cokra_protocol::ThreadId;
use cokra_protocol::TurnAbortedEvent;
use cokra_protocol::UserInput as ProtocolUserInput;
use cokra_state::StateDb;

use crate::agent::AgentControl;
use crate::agent::AgentStatus;
use crate::agent::Turn;
use crate::agent::team_runtime::build_leader_agent_teams_prompt_suffix;
use crate::agent::team_runtime::clear_team_runtime;
use crate::agent::team_runtime::register_team_runtime;
use crate::agent::team_runtime::runtime_for_thread;
use crate::model::ChatResponse;
use crate::model::ModelClient;
use crate::model::ToolCall;
use crate::model::Usage;
use crate::model::init_model_layer;
use crate::session::Session;
use crate::thread_manager::ThreadManager;
use crate::tool_runtime::UnifiedToolRuntime;
use crate::tools::build_default_tooling_with_cwd;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolContext;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolRegistry;
use crate::tools::router::ToolRouter;
use crate::tools::router::ToolRunContext;
use crate::turn::TurnConfig;

pub(crate) const SUBMISSION_CHANNEL_CAPACITY: usize = 64;
const ROOT_THREAD_ID_STATE_KEY_SUFFIX: &str = "::root_thread_id";

/// Turn runtime state snapshot.
#[derive(Debug, Default)]
pub struct TurnState {
  pub turns_executed: u64,
  pub current_input: Option<String>,
  pub last_output: Option<String>,
}

/// Stream event for simplified streaming mode.
#[derive(Debug, Clone)]
pub enum StreamEvent {
  Started,
  Delta(String),
  Completed(TurnResult),
  Error(String),
}

/// User-facing result of one turn.
#[derive(Debug, Clone)]
pub struct TurnResult {
  pub final_message: String,
  pub usage: Usage,
  pub success: bool,
}

/// Main Cokra orchestrator.
///
/// The interface mirrors codex queue-pair semantics:
/// submit operations via `submit` and consume events via `next_event`.
pub struct Cokra {
  pub(crate) tx_sub: mpsc::Sender<Submission>,
  pub(crate) rx_event: Arc<Mutex<mpsc::Receiver<Event>>>,
  pub(crate) agent_status: watch::Receiver<AgentStatus>,
  pub(crate) session: Arc<Session>,

  #[allow(dead_code)]
  pub(crate) model_client: Arc<ModelClient>,
  #[allow(dead_code)]
  pub(crate) tool_context: Arc<ToolContext>,
  pub(crate) config: Arc<Config>,
  pub(crate) turn_state: Arc<RwLock<TurnState>>,
  pub(crate) event_bus: Arc<broadcast::Sender<EventMsg>>,
  #[allow(dead_code)]
  pub(crate) tool_registry: Arc<ToolRegistry>,
  pub(crate) tool_router: Arc<ToolRouter>,
  #[allow(dead_code)]
  pub(crate) tool_runtime: Arc<UnifiedToolRuntime>,
  pub(crate) agent_control: Arc<AgentControl>,
  pub(crate) thread_manager: Arc<ThreadManager>,
}

/// Result of spawning a Cokra runtime.
pub struct CokraSpawnOk {
  pub cokra: Cokra,
  pub thread_id: cokra_protocol::ThreadId,
}

impl Cokra {
  pub async fn new(config: Config) -> anyhow::Result<Self> {
    Ok(Self::spawn(config).await?.cokra)
  }

  pub async fn spawn(config: Config) -> anyhow::Result<CokraSpawnOk> {
    let model_client = init_model_layer(&config)
      .await
      .context("failed to initialize model layer")?;
    Self::spawn_with_model_client(config, model_client).await
  }

  pub async fn new_with_model_client(
    config: Config,
    model_client: Arc<ModelClient>,
  ) -> anyhow::Result<Self> {
    Ok(
      Self::spawn_with_model_client(config, model_client)
        .await?
        .cokra,
    )
  }

  pub async fn spawn_with_model_client(
    config: Config,
    model_client: Arc<ModelClient>,
  ) -> anyhow::Result<CokraSpawnOk> {
    let config = Arc::new(config);
    let (tx_sub, rx_sub) = mpsc::channel(SUBMISSION_CHANNEL_CAPACITY);
    let (tx_raw_event, rx_raw_event) = mpsc::channel(512);
    let (tx_event, rx_event) = mpsc::channel(1024);

    let root_thread_id = resolve_or_persist_root_thread_id(&config).await?;
    let session = Arc::new(Session::new_with_thread_id(root_thread_id.clone()));
    let thread_manager = Arc::new(ThreadManager::new(root_thread_id.clone()));
    let guards = Arc::new(crate::agent::Guards::default());
    let turn_config = build_turn_config(&config);

    let configured_provider = config.models.provider.clone();
    if !configured_provider.is_empty()
      && !model_client
        .registry()
        .has_provider(&configured_provider)
        .await
    {
      anyhow::bail!(
        "Configured provider '{}' is not available. Set provider credentials (for example OPENAI_API_KEY) or switch provider with `-c models.provider=ollama` and run Ollama.",
        configured_provider
      );
    }

    let tooling = build_default_tooling_with_cwd(&config, &config.cwd).await?;
    let tool_registry = tooling.registry.clone();
    let tool_router = tooling.router.clone();
    let tool_runtime = tooling.runtime.clone();
    let agent_control = Arc::new(AgentControl::new(
      Uuid::new_v4().to_string(),
      model_client.clone(),
      tool_registry.clone(),
      tool_router.clone(),
      tool_runtime.clone(),
      session.clone(),
      turn_config,
      tx_raw_event.clone(),
      thread_manager.downgrade_state(),
      guards,
      root_thread_id.clone(),
    ));
    register_team_runtime(
      config.clone(),
      model_client.clone(),
      agent_control.clone(),
      agent_control.guards(),
      thread_manager.state(),
      tx_raw_event.clone(),
      root_thread_id,
    )
    .await?;
    agent_control.start().await?;
    let agent_status = agent_control.subscribe_status();

    let (event_bus, _event_rx) = broadcast::channel(1024);
    let event_bus = Arc::new(event_bus);
    let thread_id = session.thread_id().cloned().unwrap_or_default();

    // Forward internal turn/tool events into public queue-pair events.
    tokio::spawn(forward_internal_events(
      rx_raw_event,
      tx_event.clone(),
      event_bus.clone(),
    ));

    // Emit initial session configured event, matching codex startup behavior.
    emit_event(
      &tx_event,
      &event_bus,
      EventMsg::SessionConfigured(SessionConfiguredEvent {
        thread_id: thread_id.to_string(),
        model: build_turn_config(&config).model,
        approval_policy: format!("{:?}", config.approval.policy),
        sandbox_mode: format!("{:?}", config.sandbox.mode),
      }),
    )
    .await;

    // Submission loop runs until Op::Shutdown.
    tokio::spawn(submission_loop(
      session.clone(),
      config.clone(),
      agent_control.clone(),
      rx_sub,
      tx_event.clone(),
      event_bus.clone(),
    ));

    let cokra = Cokra {
      tx_sub,
      rx_event: Arc::new(Mutex::new(rx_event)),
      agent_status,
      session,
      model_client,
      tool_context: Arc::new(ToolContext {
        cwd: config.cwd.clone(),
      }),
      config,
      turn_state: Arc::new(RwLock::new(TurnState::default())),
      event_bus,
      tool_registry,
      tool_router,
      tool_runtime,
      agent_control,
      thread_manager,
    };

    Ok(CokraSpawnOk { cokra, thread_id })
  }

  /// Submit an operation and return generated submission id.
  pub async fn submit(&self, op: Op) -> anyhow::Result<String> {
    let id = Uuid::new_v4().to_string();
    let sub = Submission { id: id.clone(), op };
    self.submit_with_id(sub).await?;
    Ok(id)
  }

  /// Submit with explicit id (used by tests / compatibility callers).
  pub async fn submit_with_id(&self, sub: Submission) -> anyhow::Result<()> {
    self
      .tx_sub
      .send(sub)
      .await
      .map_err(|_| anyhow::anyhow!("internal agent loop terminated"))?;
    Ok(())
  }

  /// Consume the next emitted event from queue pair.
  pub async fn next_event(&self) -> anyhow::Result<Event> {
    let mut rx = self.rx_event.lock().await;
    rx.recv()
      .await
      .ok_or_else(|| anyhow::anyhow!("internal agent loop terminated"))
  }

  /// Convenience helper for CLI path; internally runs through queue pair.
  pub async fn run_turn(&self, user_message: String) -> anyhow::Result<TurnResult> {
    {
      let mut state = self.turn_state.write().await;
      state.current_input = Some(user_message.clone());
    }

    let cwd = self.config.cwd.clone();
    let op = Op::UserTurn {
      items: vec![ProtocolUserInput::Text {
        text: user_message,
        text_elements: Vec::new(),
      }],
      cwd,
      approval_policy: map_approval_policy(&self.config),
      sandbox_policy: map_sandbox_policy(&self.config),
      model: build_turn_config(&self.config).model,
      effort: None,
      summary: None,
      final_output_json_schema: None,
      collaboration_mode: None,
      personality: None,
    };
    let _sub_id = self.submit(op).await?;

    let mut final_message = String::new();
    loop {
      let event = self.next_event().await?;
      match event.msg {
        EventMsg::AgentMessageDelta(delta) | EventMsg::AgentMessageContentDelta(delta) => {
          final_message.push_str(&delta.delta);
        }
        EventMsg::ItemCompleted(item) => {
          if let cokra_protocol::TurnItem::AgentMessage(agent) = item.item {
            let text = agent
              .content
              .iter()
              .map(|part| match part {
                cokra_protocol::AgentMessageContent::Text { text } => text.as_str(),
              })
              .collect::<String>();
            if !text.is_empty() {
              final_message = text;
            }
          }
        }
        EventMsg::TurnComplete(done) => {
          let success = matches!(done.status, CompletionStatus::Success);
          let result = TurnResult {
            final_message,
            usage: Usage::default(),
            success,
          };
          {
            let mut state = self.turn_state.write().await;
            state.turns_executed += 1;
            state.last_output = Some(result.final_message.clone());
            state.current_input = None;
          }
          return Ok(result);
        }
        EventMsg::TurnAborted(aborted) => {
          return Err(anyhow::anyhow!("turn aborted: {}", aborted.reason));
        }
        EventMsg::Error(err) => {
          return Err(anyhow::anyhow!("{}", err.user_facing_message));
        }
        _ => {}
      }
    }
  }

  pub async fn process_llm_response(
    &self,
    response: ChatResponse,
  ) -> anyhow::Result<Vec<ToolCall>> {
    let mut calls = Vec::new();
    for choice in response.choices {
      if let Some(tool_calls) = choice.message.tool_calls {
        calls.extend(tool_calls);
      }
    }
    Ok(calls)
  }

  pub async fn execute_tool(&self, tool_call: ToolCall) -> Result<ToolOutput, FunctionCallError> {
    let args = serde_json::from_str::<serde_json::Value>(&tool_call.function.arguments)
      .map_err(|e| FunctionCallError::RespondToModel(format!("invalid tool args: {e}")))?;

    let thread_id = self
      .session
      .thread_id()
      .cloned()
      .unwrap_or_default()
      .to_string();
    let mut run_ctx = ToolRunContext::new(
      Arc::clone(&self.session),
      thread_id,
      Uuid::new_v4().to_string(),
      self.config.cwd.clone(),
      map_approval_policy(&self.config),
      map_sandbox_policy(&self.config),
    );
    run_ctx.has_managed_network_requirements = self.config.sandbox.network_access;
    run_ctx.allowed_domains = Vec::new();
    run_ctx.denied_domains = Vec::new();

    self
      .tool_router
      .route_tool_call(&tool_call.function.name, args, run_ctx)
      .await
  }

  pub async fn run_turn_stream(
    &self,
    user_message: String,
  ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
    let stream = async_stream::stream! {
      yield StreamEvent::Started;
      match self.run_turn(user_message).await {
        Ok(result) => {
          if !result.final_message.is_empty() {
            yield StreamEvent::Delta(result.final_message.clone());
          }
          yield StreamEvent::Completed(result);
        }
        Err(err) => {
          yield StreamEvent::Error(err.to_string());
        }
      }
    };

    Box::pin(stream)
  }

  pub fn subscribe_events(&self) -> broadcast::Receiver<EventMsg> {
    self.event_bus.subscribe()
  }

  pub fn agent_status(&self) -> AgentStatus {
    self.agent_status.borrow().clone()
  }

  pub fn thread_id(&self) -> Option<&cokra_protocol::ThreadId> {
    self.session.thread_id()
  }

  /// Access the model client (for listing providers/models in the TUI).
  pub fn model_client(&self) -> &Arc<ModelClient> {
    &self.model_client
  }

  pub fn cwd(&self) -> PathBuf {
    self.config.cwd.clone()
  }

  pub fn config(&self) -> &Config {
    &self.config
  }

  pub fn config_layer_stack(&self) -> Option<cokra_config::ConfigLayerStack> {
    self.config.config_layer_stack.clone()
  }

  pub fn list_thread_ids(&self) -> Vec<cokra_protocol::ThreadId> {
    self.thread_manager.list_thread_ids()
  }

  pub fn team_snapshot(&self) -> Option<cokra_protocol::TeamSnapshot> {
    let thread_id = self.thread_id()?.to_string();
    let runtime = runtime_for_thread(&thread_id)?;
    Some(runtime.snapshot())
  }

  pub async fn cleanup_team_runtime(&self) -> anyhow::Result<()> {
    let Some(thread_id) = self.thread_id().map(ToString::to_string) else {
      return Ok(());
    };
    let Some(runtime) = runtime_for_thread(&thread_id) else {
      return Ok(());
    };
    for agent_id in runtime.list_spawned_agent_ids() {
      let _ = runtime.close_agent(&agent_id).await;
    }
    runtime.clear_state().await;
    Ok(())
  }

  pub async fn shutdown(self) -> anyhow::Result<()> {
    let _ = self.submit(Op::Shutdown).await?;
    self.agent_control.stop().await?;
    self.session.shutdown().await?;
    if let Some(thread_id) = self.thread_id() {
      clear_team_runtime(thread_id);
    }
    Ok(())
  }
}

impl Drop for Cokra {
  fn drop(&mut self) {
    if let Some(thread_id) = self.thread_id() {
      clear_team_runtime(thread_id);
    }
  }
}

async fn resolve_or_persist_root_thread_id(config: &Arc<Config>) -> anyhow::Result<ThreadId> {
  let state_db = StateDb::new(StateDb::default_path_for(&config.cwd)).await?;
  let store_key = config.cwd.display().to_string();
  let key = format!("{store_key}{ROOT_THREAD_ID_STATE_KEY_SUFFIX}");

  if let Some(thread_id) = state_db.load_json::<ThreadId>(&key).await? {
    return Ok(thread_id);
  }

  // Migration: infer the prior root thread id from persisted team state when possible.
  if let Some(inferred) = infer_root_thread_id_from_team_state(&state_db, &store_key).await? {
    state_db.save_json(&key, &inferred).await?;
    return Ok(inferred);
  }

  let thread_id = ThreadId::new();
  state_db.save_json(&key, &thread_id).await?;
  Ok(thread_id)
}

async fn infer_root_thread_id_from_team_state(
  state_db: &StateDb,
  store_key: &str,
) -> anyhow::Result<Option<ThreadId>> {
  let Some(team_state) = state_db
    .load_json::<crate::agent::team_state::TeamState>(store_key)
    .await?
  else {
    return Ok(None);
  };

  Ok(
    team_state
      .likely_root_thread_id()
      .and_then(|id| ThreadId::parse(&id)),
  )
}

fn build_turn_config(config: &Config) -> TurnConfig {
  let provider = config.models.provider.trim();
  let model = config.models.model.trim();

  let resolved_model = resolve_model_id(provider, model);
  let mut system_prompt = build_system_prompt();
  if config.agents.max_threads > 1 {
    system_prompt.push_str(build_leader_agent_teams_prompt_suffix());
  }

  TurnConfig {
    model: resolved_model,
    system_prompt: Some(system_prompt),
    approval_policy: map_approval_policy(config),
    sandbox_policy: map_sandbox_policy(config),
    cwd: config.cwd.clone(),
    has_managed_network_requirements: config.sandbox.network_access,
    allowed_domains: Vec::new(),
    denied_domains: Vec::new(),
    ..TurnConfig::default()
  }
}

/// Build the system prompt for the LLM.
///
/// Prompt text lives in `src/prompts/base.md` and is embedded at compile time.
/// Edit that file directly to change agent behavior — no Rust changes needed.
/// See `crate::prompts` for the full file layout.
fn build_system_prompt() -> String {
  crate::prompts::BASE.to_string()
}

fn resolve_model_id(provider: &str, model: &str) -> String {
  if provider.is_empty() || model.is_empty() {
    return model.to_string();
  }

  let provider_prefix = format!("{provider}/");
  if model.starts_with(&provider_prefix) {
    return model.to_string();
  }

  format!("{provider}/{model}")
}

fn map_approval_policy(config: &Config) -> AskForApproval {
  match config.approval.policy {
    ApprovalMode::Ask => AskForApproval::OnRequest,
    ApprovalMode::Auto => AskForApproval::UnlessTrusted,
    ApprovalMode::Never => AskForApproval::Never,
  }
}

fn map_sandbox_policy(config: &Config) -> SandboxPolicy {
  match config.sandbox.mode {
    SandboxMode::Strict => SandboxPolicy::ReadOnly {
      access: ReadOnlyAccess::FullAccess,
    },
    SandboxMode::Permissive => SandboxPolicy::WorkspaceWrite {
      writable_roots: vec![config.cwd.display().to_string()],
      read_only_access: ReadOnlyAccess::FullAccess,
      network_access: config.sandbox.network_access,
      exclude_tmpdir_env_var: false,
      exclude_slash_tmp: false,
    },
    SandboxMode::DangerFullAccess => SandboxPolicy::DangerFullAccess,
  }
}

fn extract_text_from_items(items: &[ProtocolUserInput]) -> String {
  items
    .iter()
    .filter_map(|item| match item {
      ProtocolUserInput::Text { text, .. } => Some(text.as_str()),
      _ => None,
    })
    .collect::<Vec<_>>()
    .join("\n")
}

async fn forward_internal_events(
  mut rx_raw_event: mpsc::Receiver<EventMsg>,
  tx_event: mpsc::Sender<Event>,
  event_bus: Arc<broadcast::Sender<EventMsg>>,
) {
  while let Some(msg) = rx_raw_event.recv().await {
    emit_event(&tx_event, &event_bus, msg).await;
  }
}

async fn emit_event(
  tx_event: &mpsc::Sender<Event>,
  event_bus: &broadcast::Sender<EventMsg>,
  msg: EventMsg,
) {
  let legacy = legacy_events_for(&msg);
  let _ = event_bus.send(msg.clone());
  let _ = tx_event
    .send(Event {
      id: Uuid::new_v4().to_string(),
      msg,
    })
    .await;

  for legacy_msg in legacy {
    let _ = event_bus.send(legacy_msg.clone());
    let _ = tx_event
      .send(Event {
        id: Uuid::new_v4().to_string(),
        msg: legacy_msg,
      })
      .await;
  }
}

fn legacy_events_for(msg: &EventMsg) -> Vec<EventMsg> {
  match msg {
    EventMsg::ItemCompleted(event) => event
      .item
      .as_legacy_events(&event.thread_id, &event.turn_id),
    _ => Vec::new(),
  }
}

async fn submission_loop(
  session: Arc<Session>,
  config: Arc<Config>,
  agent_control: Arc<AgentControl>,
  mut rx_sub: mpsc::Receiver<Submission>,
  tx_event: mpsc::Sender<Event>,
  event_bus: Arc<broadcast::Sender<EventMsg>>,
) {
  let mut queue: VecDeque<Submission> = VecDeque::new();
  let mut turn_config = build_turn_config(&config);

  loop {
    let sub = if let Some(next) = queue.pop_front() {
      next
    } else if let Some(next) = rx_sub.recv().await {
      next
    } else {
      break;
    };

    match sub.op {
      Op::ConfigureSession {
        cwd,
        approval_policy,
        sandbox_policy,
        model,
      } => {
        let approval_policy_str = format!("{approval_policy:?}");
        let sandbox_mode_str = format!("{sandbox_policy:?}");
        turn_config.model = model.clone();
        turn_config.approval_policy = approval_policy;
        turn_config.sandbox_policy = sandbox_policy;
        turn_config.cwd = cwd;
        agent_control.set_turn_config(turn_config.clone()).await;
        emit_event(
          &tx_event,
          &event_bus,
          EventMsg::SessionConfigured(SessionConfiguredEvent {
            thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
            model,
            approval_policy: approval_policy_str,
            sandbox_mode: sandbox_mode_str,
          }),
        )
        .await;
      }
      Op::OverrideTurnContext {
        cwd,
        approval_policy,
        sandbox_policy,
        model,
        collaboration_mode: _,
        personality: _,
      } => {
        if let Some(model) = model {
          turn_config.model = model;
        }
        if let Some(approval_policy) = approval_policy {
          turn_config.approval_policy = approval_policy;
        }
        if let Some(sandbox_policy) = sandbox_policy {
          turn_config.sandbox_policy = sandbox_policy;
        }
        if let Some(cwd) = cwd {
          turn_config.cwd = cwd;
        }

        agent_control.set_turn_config(turn_config.clone()).await;
        emit_event(
          &tx_event,
          &event_bus,
          EventMsg::SessionConfigured(SessionConfiguredEvent {
            thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
            model: turn_config.model.clone(),
            approval_policy: format!("{:?}", turn_config.approval_policy),
            sandbox_mode: format!("{:?}", turn_config.sandbox_policy),
          }),
        )
        .await;
      }
      Op::UserInput { items, .. } => {
        let user_item =
          cokra_protocol::TurnItem::UserMessage(cokra_protocol::UserMessageItem::new(&items));
        emit_event(
          &tx_event,
          &event_bus,
          EventMsg::ItemStarted(cokra_protocol::ItemStartedEvent {
            thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
            turn_id: sub.id.clone(),
            item: user_item.clone(),
          }),
        )
        .await;
        emit_event(
          &tx_event,
          &event_bus,
          EventMsg::ItemCompleted(cokra_protocol::ItemCompletedEvent {
            thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
            turn_id: sub.id.clone(),
            item: user_item,
          }),
        )
        .await;
        let user_message = extract_text_from_items(&items);
        run_turn_with_interrupt(
          &session,
          &agent_control,
          user_message,
          &mut rx_sub,
          &mut queue,
          &tx_event,
          &event_bus,
          &sub.id,
        )
        .await;
      }
      Op::UserTurn {
        items,
        model,
        cwd: _,
        approval_policy: _,
        sandbox_policy: _,
        effort: _,
        summary: _,
        final_output_json_schema: _,
        collaboration_mode: _,
        personality: _,
      } => {
        let user_item =
          cokra_protocol::TurnItem::UserMessage(cokra_protocol::UserMessageItem::new(&items));
        emit_event(
          &tx_event,
          &event_bus,
          EventMsg::ItemStarted(cokra_protocol::ItemStartedEvent {
            thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
            turn_id: sub.id.clone(),
            item: user_item.clone(),
          }),
        )
        .await;
        emit_event(
          &tx_event,
          &event_bus,
          EventMsg::ItemCompleted(cokra_protocol::ItemCompletedEvent {
            thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
            turn_id: sub.id.clone(),
            item: user_item,
          }),
        )
        .await;
        turn_config.model = model;
        agent_control.set_turn_config(turn_config.clone()).await;
        let user_message = extract_text_from_items(&items);
        run_turn_with_interrupt(
          &session,
          &agent_control,
          user_message,
          &mut rx_sub,
          &mut queue,
          &tx_event,
          &event_bus,
          &sub.id,
        )
        .await;
      }
      Op::ExecApproval {
        id,
        turn_id,
        decision,
      } => {
        let mut notified = session.notify_exec_approval(&id, decision.clone()).await;
        if !notified
          && let Some(root_thread_id) = session.thread_id().map(ToString::to_string)
          && let Some(team_runtime) = runtime_for_thread(&root_thread_id)
        {
          notified = team_runtime.notify_exec_approval(&id, decision).await;
        }
        if !notified {
          emit_event(
            &tx_event,
            &event_bus,
            EventMsg::Warning(cokra_protocol::WarningEvent {
              thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
              turn_id: turn_id.unwrap_or_else(|| sub.id.clone()),
              message: format!("no pending approval found for id: {id}"),
            }),
          )
          .await;
        }
      }
      Op::UserInputAnswer { id, response } => {
        let mut notified = session.notify_user_input(&id, response.clone()).await;
        if !notified
          && let Some(root_thread_id) = session.thread_id().map(ToString::to_string)
          && let Some(team_runtime) = runtime_for_thread(&root_thread_id)
        {
          notified = team_runtime.notify_user_input(&id, response).await;
        }
        if !notified {
          emit_event(
            &tx_event,
            &event_bus,
            EventMsg::Warning(cokra_protocol::WarningEvent {
              thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
              turn_id: sub.id.clone(),
              message: format!("no pending user input found for id: {id}"),
            }),
          )
          .await;
        }
      }
      Op::Interrupt => {
        emit_event(
          &tx_event,
          &event_bus,
          EventMsg::TurnAborted(TurnAbortedEvent {
            thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
            turn_id: sub.id,
            reason: "no active turn".to_string(),
          }),
        )
        .await;
      }
      Op::Shutdown => {
        emit_event(&tx_event, &event_bus, EventMsg::ShutdownComplete).await;
        break;
      }
      _ => {
        emit_event(
          &tx_event,
          &event_bus,
          EventMsg::Warning(cokra_protocol::WarningEvent {
            thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
            turn_id: sub.id,
            message: "operation not implemented in phase 1 loop".to_string(),
          }),
        )
        .await;
      }
    }
  }
}

#[allow(clippy::too_many_arguments)]
async fn run_turn_with_interrupt(
  session: &Session,
  agent_control: &AgentControl,
  user_message: String,
  rx_sub: &mut mpsc::Receiver<Submission>,
  queue: &mut VecDeque<Submission>,
  tx_event: &mpsc::Sender<Event>,
  event_bus: &broadcast::Sender<EventMsg>,
  turn_id: &str,
) {
  if user_message.trim().is_empty() {
    emit_event(
      tx_event,
      event_bus,
      EventMsg::Warning(cokra_protocol::WarningEvent {
        thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
        turn_id: turn_id.to_string(),
        message: "empty input ignored".to_string(),
      }),
    )
    .await;
    return;
  }

  let mut fut = Box::pin(agent_control.process_turn(Turn {
    turn_id: turn_id.to_string(),
    user_message,
  }));
  loop {
    tokio::select! {
      res = &mut fut => {
        let _ = res;
        session.clear_pending_approvals_for_turn(turn_id).await;
        session.clear_pending_user_inputs_for_turn(turn_id).await;
        break;
      }
      maybe_sub = rx_sub.recv() => {
        let Some(next_sub) = maybe_sub else {
          session.clear_pending_approvals_for_turn(turn_id).await;
          session.clear_pending_user_inputs_for_turn(turn_id).await;
          break;
        };
        match next_sub.op {
          Op::ExecApproval {
            id,
            turn_id: op_turn_id,
            decision,
          } => {
            let mut notified = session.notify_exec_approval(&id, decision.clone()).await;
            if !notified
              && let Some(root_thread_id) = session.thread_id().map(ToString::to_string)
              && let Some(team_runtime) = runtime_for_thread(&root_thread_id)
            {
              notified = team_runtime.notify_exec_approval(&id, decision).await;
            }
            if !notified {
              emit_event(
                tx_event,
                event_bus,
                EventMsg::Warning(cokra_protocol::WarningEvent {
                  thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
                  turn_id: op_turn_id.unwrap_or_else(|| turn_id.to_string()),
                  message: format!("no pending approval found for id: {id}"),
                }),
              ).await;
            }
          }
          Op::UserInputAnswer { id, response } => {
            let mut notified = session.notify_user_input(&id, response.clone()).await;
            if !notified
              && let Some(root_thread_id) = session.thread_id().map(ToString::to_string)
              && let Some(team_runtime) = runtime_for_thread(&root_thread_id)
            {
              notified = team_runtime.notify_user_input(&id, response).await;
            }
            if !notified {
              emit_event(
                tx_event,
                event_bus,
                EventMsg::Warning(cokra_protocol::WarningEvent {
                  thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
                  turn_id: turn_id.to_string(),
                  message: format!("no pending user input found for id: {id}"),
                }),
              ).await;
            }
          }
          Op::Interrupt => {
            emit_event(
              tx_event,
              event_bus,
              EventMsg::TurnAborted(TurnAbortedEvent {
                thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
                turn_id: turn_id.to_string(),
                reason: "interrupted".to_string(),
              }),
            ).await;
            session.clear_pending_approvals_for_turn(turn_id).await;
            session.clear_pending_user_inputs_for_turn(turn_id).await;
            break;
          }
          Op::Shutdown => {
            emit_event(tx_event, event_bus, EventMsg::ShutdownComplete).await;
            session.clear_pending_approvals_for_turn(turn_id).await;
            session.clear_pending_user_inputs_for_turn(turn_id).await;
            break;
          }
          _ => queue.push_back(next_sub),
        }
      }
    }
  }
}

impl CokraSpawnOk {
  pub fn thread_id(&self) -> Option<&cokra_protocol::ThreadId> {
    self.cokra.thread_id()
  }
}

#[cfg(test)]
mod tests {
  use std::pin::Pin;
  use std::sync::Arc;
  use std::sync::Mutex;
  use std::time::Duration;

  use async_trait::async_trait;
  use futures::Stream;
  use reqwest::Client;
  use tokio::time::timeout;
  use uuid::Uuid;

  use super::Cokra;
  use super::map_sandbox_policy;
  use super::resolve_model_id;
  use crate::agent::team_runtime::runtime_for_thread;
  use crate::model::ChatRequest;
  use crate::model::ChatResponse;
  use crate::model::Choice;
  use crate::model::ChoiceMessage;
  use crate::model::Chunk;
  use crate::model::ContentDelta;
  use crate::model::ListModelsResponse;
  use crate::model::Message;
  use crate::model::ModelClient;
  use crate::model::ModelInfo;
  use crate::model::ProviderConfig;
  use crate::model::ProviderRegistry;
  use crate::model::ToolCall;
  use crate::model::ToolCallDelta;
  use crate::model::ToolCallFunction;
  use crate::model::Usage;
  use crate::model::provider::ModelProvider;
  use crate::tools::context::FunctionCallError;
  use crate::tools::context::ToolOutput;
  use crate::tools::router::ToolRunContext;
  use cokra_config::ApprovalMode;
  use cokra_protocol::AskForApproval;
  use cokra_protocol::CompletionStatus;
  use cokra_protocol::EventMsg;
  use cokra_protocol::Op;
  use cokra_protocol::ReviewDecision;
  use cokra_protocol::TeamSnapshot;
  use cokra_protocol::TeamTask;
  use cokra_protocol::TeamTaskStatus;
  use cokra_protocol::UserInput;

  #[derive(Debug)]
  struct MockProvider {
    client: Client,
    config: ProviderConfig,
  }

  impl MockProvider {
    fn new() -> Self {
      Self {
        client: Client::new(),
        config: ProviderConfig {
          provider_id: "mock".to_string(),
          ..Default::default()
        },
      }
    }
  }

  #[async_trait]
  impl ModelProvider for MockProvider {
    fn provider_id(&self) -> &'static str {
      "mock"
    }

    fn provider_name(&self) -> &'static str {
      "Mock Provider"
    }

    async fn chat_completion(&self, _request: ChatRequest) -> crate::model::Result<ChatResponse> {
      Ok(ChatResponse {
        id: "mock-response".to_string(),
        object_type: "chat.completion".to_string(),
        created: 0,
        model: "mock/default".to_string(),
        choices: vec![Choice {
          index: 0,
          message: ChoiceMessage {
            role: "assistant".to_string(),
            content: Some("mock reply".to_string()),
            tool_calls: None,
          },
          finish_reason: Some("stop".to_string()),
        }],
        usage: Usage {
          input_tokens: 1,
          output_tokens: 1,
          total_tokens: 2,
        },
        extra: Default::default(),
      })
    }

    async fn chat_completion_stream(
      &self,
      _request: ChatRequest,
    ) -> crate::model::Result<Pin<Box<dyn Stream<Item = crate::model::Result<Chunk>> + Send>>> {
      Ok(Box::pin(futures::stream::iter(vec![
        Ok(Chunk::Content {
          delta: ContentDelta {
            text: "mock reply".to_string(),
          },
        }),
        Ok(Chunk::MessageStop),
      ])))
    }

    async fn list_models(&self) -> crate::model::Result<ListModelsResponse> {
      Ok(ListModelsResponse {
        object_type: "list".to_string(),
        data: vec![ModelInfo {
          id: "mock/default".to_string(),
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

  async fn build_mock_client() -> Arc<ModelClient> {
    let registry = Arc::new(ProviderRegistry::new());
    registry.register(MockProvider::new()).await;
    registry
      .set_default("mock")
      .await
      .expect("set mock default");
    Arc::new(
      ModelClient::new(registry)
        .await
        .expect("build model client"),
    )
  }

  #[derive(Debug)]
  struct MockOpenRouterProvider {
    client: Client,
    config: ProviderConfig,
  }

  impl MockOpenRouterProvider {
    fn new() -> Self {
      Self {
        client: Client::new(),
        config: ProviderConfig {
          provider_id: "openrouter".to_string(),
          ..Default::default()
        },
      }
    }
  }

  #[async_trait]
  impl ModelProvider for MockOpenRouterProvider {
    fn provider_id(&self) -> &'static str {
      "openrouter"
    }

    fn provider_name(&self) -> &'static str {
      "Mock OpenRouter Provider"
    }

    fn default_models(&self) -> Vec<&'static str> {
      vec!["openai/gpt-4o-mini"]
    }

    async fn chat_completion(&self, request: ChatRequest) -> crate::model::Result<ChatResponse> {
      if request.model != "openai/gpt-4o-mini" {
        return Err(crate::model::ModelError::InvalidRequest(format!(
          "expected nested model id, got {}",
          request.model
        )));
      }

      Ok(ChatResponse {
        id: "mock-openrouter-response".to_string(),
        object_type: "chat.completion".to_string(),
        created: 0,
        model: "openai/gpt-4o-mini".to_string(),
        choices: vec![Choice {
          index: 0,
          message: ChoiceMessage {
            role: "assistant".to_string(),
            content: Some("openrouter nested model ok".to_string()),
            tool_calls: None,
          },
          finish_reason: Some("stop".to_string()),
        }],
        usage: Usage {
          input_tokens: 1,
          output_tokens: 1,
          total_tokens: 2,
        },
        extra: Default::default(),
      })
    }

    async fn chat_completion_stream(
      &self,
      request: ChatRequest,
    ) -> crate::model::Result<Pin<Box<dyn Stream<Item = crate::model::Result<Chunk>> + Send>>> {
      if request.model != "openai/gpt-4o-mini" {
        return Err(crate::model::ModelError::InvalidRequest(format!(
          "expected nested model id, got {}",
          request.model
        )));
      }

      Ok(Box::pin(futures::stream::iter(vec![
        Ok(Chunk::Content {
          delta: ContentDelta {
            text: "openrouter nested model ok".to_string(),
          },
        }),
        Ok(Chunk::MessageStop),
      ])))
    }

    async fn list_models(&self) -> crate::model::Result<ListModelsResponse> {
      Ok(ListModelsResponse {
        object_type: "list".to_string(),
        data: vec![ModelInfo {
          id: "openai/gpt-4o-mini".to_string(),
          object_type: "model".to_string(),
          created: 0,
          owned_by: Some("openrouter".to_string()),
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

  async fn build_openrouter_mock_client() -> Arc<ModelClient> {
    let registry = Arc::new(ProviderRegistry::new());
    registry.register(MockOpenRouterProvider::new()).await;
    registry
      .set_default("openrouter")
      .await
      .expect("set openrouter default");
    Arc::new(
      ModelClient::new(registry)
        .await
        .expect("build model client"),
    )
  }

  #[derive(Debug)]
  struct MockToolLoopProvider {
    client: Client,
    config: ProviderConfig,
    file_path: String,
    calls: Arc<Mutex<u32>>,
  }

  impl MockToolLoopProvider {
    fn new(file_path: String) -> Self {
      Self {
        client: Client::new(),
        config: ProviderConfig {
          provider_id: "mocktool".to_string(),
          ..Default::default()
        },
        file_path,
        calls: Arc::new(Mutex::new(0)),
      }
    }
  }

  #[async_trait]
  impl ModelProvider for MockToolLoopProvider {
    fn provider_id(&self) -> &'static str {
      "mocktool"
    }

    fn provider_name(&self) -> &'static str {
      "Mock Tool Loop Provider"
    }

    async fn chat_completion(&self, request: ChatRequest) -> crate::model::Result<ChatResponse> {
      let mut calls = self
        .calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
      *calls += 1;

      if *calls == 1 {
        return Ok(ChatResponse {
          id: "mock-tool-1".to_string(),
          object_type: "chat.completion".to_string(),
          created: 0,
          model: "mocktool/default".to_string(),
          choices: vec![Choice {
            index: 0,
            message: ChoiceMessage {
              role: "assistant".to_string(),
              content: None,
              tool_calls: Some(vec![ToolCall {
                id: "call_read_1".to_string(),
                call_type: "function".to_string(),
                function: ToolCallFunction {
                  name: "read_file".to_string(),
                  arguments: serde_json::json!({
                    "file_path": self.file_path
                  })
                  .to_string(),
                },
                provider_meta: None,
              }]),
            },
            finish_reason: Some("tool_calls".to_string()),
          }],
          usage: Usage {
            input_tokens: 2,
            output_tokens: 4,
            total_tokens: 6,
          },
          extra: Default::default(),
        });
      }

      let saw_tool_output = request.messages.iter().any(|message| {
        matches!(message, Message::Tool { tool_call_id, content }
          if tool_call_id == "call_read_1" && content.contains("hello from tool loop"))
      });

      if !saw_tool_output {
        return Err(crate::model::ModelError::InvalidRequest(
          "follow-up request missing tool output".to_string(),
        ));
      }

      Ok(ChatResponse {
        id: "mock-tool-2".to_string(),
        object_type: "chat.completion".to_string(),
        created: 0,
        model: "mocktool/default".to_string(),
        choices: vec![Choice {
          index: 0,
          message: ChoiceMessage {
            role: "assistant".to_string(),
            content: Some("tool loop complete".to_string()),
            tool_calls: None,
          },
          finish_reason: Some("stop".to_string()),
        }],
        usage: Usage {
          input_tokens: 3,
          output_tokens: 2,
          total_tokens: 5,
        },
        extra: Default::default(),
      })
    }

    async fn chat_completion_stream(
      &self,
      request: ChatRequest,
    ) -> crate::model::Result<Pin<Box<dyn Stream<Item = crate::model::Result<Chunk>> + Send>>> {
      let mut calls = self
        .calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
      *calls += 1;

      if *calls == 1 {
        let arguments = serde_json::json!({ "file_path": self.file_path }).to_string();
        return Ok(Box::pin(futures::stream::iter(vec![
          Ok(Chunk::ToolCall {
            delta: ToolCallDelta {
              id: Some("call_read_1".to_string()),
              name: Some("read_file".to_string()),
              arguments: Some(arguments),
              thought_signature: None,
            },
          }),
          Ok(Chunk::MessageStop),
        ])));
      }

      let saw_tool_output = request.messages.iter().any(|message| {
        matches!(message, Message::Tool { tool_call_id, content }
          if tool_call_id == "call_read_1" && content.contains("hello from tool loop"))
      });

      if !saw_tool_output {
        return Err(crate::model::ModelError::InvalidRequest(
          "follow-up request missing tool output".to_string(),
        ));
      }

      Ok(Box::pin(futures::stream::iter(vec![
        Ok(Chunk::Content {
          delta: ContentDelta {
            text: "tool loop complete".to_string(),
          },
        }),
        Ok(Chunk::MessageStop),
      ])))
    }

    async fn list_models(&self) -> crate::model::Result<ListModelsResponse> {
      Ok(ListModelsResponse {
        object_type: "list".to_string(),
        data: vec![ModelInfo {
          id: "mocktool/default".to_string(),
          object_type: "model".to_string(),
          created: 0,
          owned_by: Some("mocktool".to_string()),
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

  async fn build_tool_loop_client(file_path: String) -> Arc<ModelClient> {
    let registry = Arc::new(ProviderRegistry::new());
    registry
      .register(MockToolLoopProvider::new(file_path))
      .await;
    registry
      .set_default("mocktool")
      .await
      .expect("set mocktool default");
    Arc::new(
      ModelClient::new(registry)
        .await
        .expect("build model client"),
    )
  }

  #[derive(Debug)]
  struct MockApprovalLoopProvider {
    client: Client,
    config: ProviderConfig,
    file_path: String,
    calls: Arc<Mutex<u32>>,
  }

  impl MockApprovalLoopProvider {
    fn new(file_path: String) -> Self {
      Self {
        client: Client::new(),
        config: ProviderConfig {
          provider_id: "mockapproval".to_string(),
          ..Default::default()
        },
        file_path,
        calls: Arc::new(Mutex::new(0)),
      }
    }
  }

  #[async_trait]
  impl ModelProvider for MockApprovalLoopProvider {
    fn provider_id(&self) -> &'static str {
      "mockapproval"
    }

    fn provider_name(&self) -> &'static str {
      "Mock Approval Loop Provider"
    }

    async fn chat_completion(&self, request: ChatRequest) -> crate::model::Result<ChatResponse> {
      let mut calls = self
        .calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
      *calls += 1;

      if *calls == 1 {
        return Ok(ChatResponse {
          id: "mock-approval-1".to_string(),
          object_type: "chat.completion".to_string(),
          created: 0,
          model: "mockapproval/default".to_string(),
          choices: vec![Choice {
            index: 0,
            message: ChoiceMessage {
              role: "assistant".to_string(),
              content: None,
              tool_calls: Some(vec![ToolCall {
                id: "call_write_1".to_string(),
                call_type: "function".to_string(),
                function: ToolCallFunction {
                  name: "write_file".to_string(),
                  arguments: serde_json::json!({
                    "file_path": self.file_path,
                    "content": "approved"
                  })
                  .to_string(),
                },
                provider_meta: None,
              }]),
            },
            finish_reason: Some("tool_calls".to_string()),
          }],
          usage: Usage {
            input_tokens: 2,
            output_tokens: 4,
            total_tokens: 6,
          },
          extra: Default::default(),
        });
      }

      let saw_tool_output = request.messages.iter().any(|message| {
        matches!(message, Message::Tool { tool_call_id, content }
          if tool_call_id == "call_write_1" && content.contains("wrote"))
      });

      if !saw_tool_output {
        return Err(crate::model::ModelError::InvalidRequest(
          "follow-up request missing write_file output".to_string(),
        ));
      }

      Ok(ChatResponse {
        id: "mock-approval-2".to_string(),
        object_type: "chat.completion".to_string(),
        created: 0,
        model: "mockapproval/default".to_string(),
        choices: vec![Choice {
          index: 0,
          message: ChoiceMessage {
            role: "assistant".to_string(),
            content: Some("approval loop complete".to_string()),
            tool_calls: None,
          },
          finish_reason: Some("stop".to_string()),
        }],
        usage: Usage {
          input_tokens: 2,
          output_tokens: 2,
          total_tokens: 4,
        },
        extra: Default::default(),
      })
    }

    async fn chat_completion_stream(
      &self,
      request: ChatRequest,
    ) -> crate::model::Result<Pin<Box<dyn Stream<Item = crate::model::Result<Chunk>> + Send>>> {
      let mut calls = self
        .calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
      *calls += 1;

      if *calls == 1 {
        let arguments = serde_json::json!({
          "file_path": self.file_path,
          "content": "approved"
        })
        .to_string();
        return Ok(Box::pin(futures::stream::iter(vec![
          Ok(Chunk::ToolCall {
            delta: ToolCallDelta {
              id: Some("call_write_1".to_string()),
              name: Some("write_file".to_string()),
              arguments: Some(arguments),
              thought_signature: None,
            },
          }),
          Ok(Chunk::MessageStop),
        ])));
      }

      let saw_tool_output = request.messages.iter().any(|message| {
        matches!(message, Message::Tool { tool_call_id, content }
          if tool_call_id == "call_write_1" && content.contains("wrote"))
      });

      if !saw_tool_output {
        return Err(crate::model::ModelError::InvalidRequest(
          "follow-up request missing write_file output".to_string(),
        ));
      }

      Ok(Box::pin(futures::stream::iter(vec![
        Ok(Chunk::Content {
          delta: ContentDelta {
            text: "approval loop complete".to_string(),
          },
        }),
        Ok(Chunk::MessageStop),
      ])))
    }

    async fn list_models(&self) -> crate::model::Result<ListModelsResponse> {
      Ok(ListModelsResponse {
        object_type: "list".to_string(),
        data: vec![ModelInfo {
          id: "mockapproval/default".to_string(),
          object_type: "model".to_string(),
          created: 0,
          owned_by: Some("mockapproval".to_string()),
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

  async fn build_approval_client(file_path: String) -> Arc<ModelClient> {
    let registry = Arc::new(ProviderRegistry::new());
    registry
      .register(MockApprovalLoopProvider::new(file_path))
      .await;
    registry
      .set_default("mockapproval")
      .await
      .expect("set mockapproval default");
    Arc::new(
      ModelClient::new(registry)
        .await
        .expect("build model client"),
    )
  }

  #[derive(Debug)]
  struct MockRequestUserInputProvider {
    client: Client,
    config: ProviderConfig,
    calls: Arc<Mutex<u32>>,
  }

  impl MockRequestUserInputProvider {
    fn new() -> Self {
      Self {
        client: Client::new(),
        config: ProviderConfig {
          provider_id: "mockinput".to_string(),
          ..Default::default()
        },
        calls: Arc::new(Mutex::new(0)),
      }
    }
  }

  #[async_trait]
  impl ModelProvider for MockRequestUserInputProvider {
    fn provider_id(&self) -> &'static str {
      "mockinput"
    }

    fn provider_name(&self) -> &'static str {
      "Mock Request User Input Provider"
    }

    async fn chat_completion(&self, request: ChatRequest) -> crate::model::Result<ChatResponse> {
      let mut calls = self
        .calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
      *calls += 1;

      if *calls == 1 {
        return Ok(ChatResponse {
          id: "mock-input-1".to_string(),
          object_type: "chat.completion".to_string(),
          created: 0,
          model: "mockinput/default".to_string(),
          choices: vec![Choice {
            index: 0,
            message: ChoiceMessage {
              role: "assistant".to_string(),
              content: None,
              tool_calls: Some(vec![ToolCall {
                id: "call_input_1".to_string(),
                call_type: "function".to_string(),
                function: ToolCallFunction {
                  name: "request_user_input".to_string(),
                  arguments: serde_json::json!({
                    "questions": [{
                      "id": "name",
                      "header": "Name",
                      "question": "What is your name?",
                      "options": [{
                        "label": "leex",
                        "description": "Use the preferred name leex."
                      }, {
                        "label": "other",
                        "description": "Provide a different answer."
                      }]
                    }]
                  })
                  .to_string(),
                },
                provider_meta: None,
              }]),
            },
            finish_reason: Some("tool_calls".to_string()),
          }],
          usage: Usage {
            input_tokens: 2,
            output_tokens: 4,
            total_tokens: 6,
          },
          extra: Default::default(),
        });
      }

      let saw_tool_output = request.messages.iter().any(|message| match message {
        Message::Tool {
          tool_call_id,
          content,
        } if tool_call_id == "call_input_1" => serde_json::from_str::<serde_json::Value>(content)
          .ok()
          .is_some_and(|value| {
            value
              == serde_json::json!({
                "answers": {
                  "name": { "answers": ["leex"] }
                }
              })
          }),
        _ => false,
      });

      if !saw_tool_output {
        return Err(crate::model::ModelError::InvalidRequest(
          "follow-up request missing request_user_input output".to_string(),
        ));
      }

      Ok(ChatResponse {
        id: "mock-input-2".to_string(),
        object_type: "chat.completion".to_string(),
        created: 0,
        model: "mockinput/default".to_string(),
        choices: vec![Choice {
          index: 0,
          message: ChoiceMessage {
            role: "assistant".to_string(),
            content: Some("user input loop complete".to_string()),
            tool_calls: None,
          },
          finish_reason: Some("stop".to_string()),
        }],
        usage: Usage {
          input_tokens: 2,
          output_tokens: 2,
          total_tokens: 4,
        },
        extra: Default::default(),
      })
    }

    async fn chat_completion_stream(
      &self,
      request: ChatRequest,
    ) -> crate::model::Result<Pin<Box<dyn Stream<Item = crate::model::Result<Chunk>> + Send>>> {
      let mut calls = self
        .calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
      *calls += 1;

      if *calls == 1 {
        let arguments = serde_json::json!({
          "questions": [{
            "id": "name",
            "header": "Name",
            "question": "What is your name?",
            "options": [{
              "label": "leex",
              "description": "Use the preferred name leex."
            }, {
              "label": "other",
              "description": "Provide a different answer."
            }]
          }]
        })
        .to_string();
        return Ok(Box::pin(futures::stream::iter(vec![
          Ok(Chunk::ToolCall {
            delta: ToolCallDelta {
              id: Some("call_input_1".to_string()),
              name: Some("request_user_input".to_string()),
              arguments: Some(arguments),
              thought_signature: None,
            },
          }),
          Ok(Chunk::MessageStop),
        ])));
      }

      let saw_tool_output = request.messages.iter().any(|message| match message {
        Message::Tool {
          tool_call_id,
          content,
        } if tool_call_id == "call_input_1" => serde_json::from_str::<serde_json::Value>(content)
          .ok()
          .is_some_and(|value| {
            value
              == serde_json::json!({
                "answers": {
                  "name": { "answers": ["leex"] }
                }
              })
          }),
        _ => false,
      });

      if !saw_tool_output {
        return Err(crate::model::ModelError::InvalidRequest(
          "follow-up request missing request_user_input output".to_string(),
        ));
      }

      Ok(Box::pin(futures::stream::iter(vec![
        Ok(Chunk::Content {
          delta: ContentDelta {
            text: "user input loop complete".to_string(),
          },
        }),
        Ok(Chunk::MessageStop),
      ])))
    }

    async fn list_models(&self) -> crate::model::Result<ListModelsResponse> {
      Ok(ListModelsResponse {
        object_type: "list".to_string(),
        data: vec![ModelInfo {
          id: "mockinput/default".to_string(),
          object_type: "model".to_string(),
          created: 0,
          owned_by: Some("mockinput".to_string()),
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

  async fn build_request_user_input_client() -> Arc<ModelClient> {
    let registry = Arc::new(ProviderRegistry::new());
    registry.register(MockRequestUserInputProvider::new()).await;
    registry
      .set_default("mockinput")
      .await
      .expect("set mockinput default");
    Arc::new(
      ModelClient::new(registry)
        .await
        .expect("build model client"),
    )
  }

  async fn execute_tool_as_thread(
    cokra: &Cokra,
    thread_id: String,
    name: &str,
    args: serde_json::Value,
  ) -> ToolOutput {
    let mut run_ctx = ToolRunContext::new(
      cokra.session.clone(),
      thread_id,
      Uuid::new_v4().to_string(),
      cokra.config.cwd.clone(),
      AskForApproval::OnFailure,
      map_sandbox_policy(&cokra.config),
    );
    run_ctx.has_managed_network_requirements = cokra.config.sandbox.network_access;

    cokra
      .tool_router
      .route_tool_call(name, args, run_ctx)
      .await
      .expect("tool call should succeed")
  }

  async fn execute_tool_as_thread_result(
    cokra: &Cokra,
    thread_id: String,
    name: &str,
    args: serde_json::Value,
  ) -> Result<ToolOutput, FunctionCallError> {
    let mut run_ctx = ToolRunContext::new(
      cokra.session.clone(),
      thread_id,
      Uuid::new_v4().to_string(),
      cokra.config.cwd.clone(),
      AskForApproval::OnFailure,
      map_sandbox_policy(&cokra.config),
    );
    run_ctx.has_managed_network_requirements = cokra.config.sandbox.network_access;

    cokra.tool_router.route_tool_call(name, args, run_ctx).await
  }

  #[tokio::test]
  async fn test_cokra_creation() {
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mock".to_string();
    config.models.model = "mock/default".to_string();
    let spawned = Cokra::spawn_with_model_client(config, build_mock_client().await)
      .await
      .expect("create cokra");
    let status = spawned.cokra.agent_status();
    assert!(matches!(
      status,
      crate::agent::AgentStatus::Ready | crate::agent::AgentStatus::PendingInit
    ));
  }

  #[tokio::test]
  async fn test_run_turn_with_mock_model() {
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mock".to_string();
    config.models.model = "mock/default".to_string();
    let cokra = Cokra::new_with_model_client(config, build_mock_client().await)
      .await
      .expect("create cokra");

    let result = cokra.run_turn("hello".to_string()).await.expect("run turn");
    assert_eq!(result.final_message, "mock reply".to_string());
    assert!(result.success);
  }

  #[tokio::test]
  async fn test_submit_and_event_stream_lifecycle() {
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mock".to_string();
    config.models.model = "mock/default".to_string();
    let cokra = Cokra::new_with_model_client(config, build_mock_client().await)
      .await
      .expect("create cokra");

    let _ = cokra
      .submit(Op::UserInput {
        items: vec![UserInput::Text {
          text: "hello".to_string(),
          text_elements: Vec::new(),
        }],
        final_output_json_schema: None,
      })
      .await
      .expect("submit");

    let mut saw_configured = false;
    let mut saw_user_message = false;
    let mut saw_started = false;
    let mut saw_item_started = false;
    let mut saw_item_completed = false;
    let mut saw_complete = false;
    let mut configured_thread_id = None;
    let mut started_thread_id = None;

    for _ in 0..20 {
      let evt = cokra.next_event().await.expect("next event");
      match evt.msg {
        EventMsg::SessionConfigured(ev) => {
          configured_thread_id = Some(ev.thread_id);
          saw_configured = true;
        }
        EventMsg::UserMessage(ev) => {
          assert_eq!(ev.items.len(), 1);
          saw_user_message = true;
        }
        EventMsg::TurnStarted(ev) => {
          started_thread_id = Some(ev.thread_id);
          saw_started = true;
        }
        EventMsg::ItemStarted(_) => saw_item_started = true,
        EventMsg::ItemCompleted(_) => saw_item_completed = true,
        EventMsg::TurnComplete(_) => {
          saw_complete = true;
          break;
        }
        _ => {}
      }
    }

    assert!(saw_configured);
    assert!(saw_user_message);
    assert!(saw_started);
    assert!(saw_item_started);
    assert!(saw_item_completed);
    assert!(saw_complete);
    assert_eq!(configured_thread_id, started_thread_id);
  }

  #[test]
  fn test_resolve_model_id_for_provider_scoped_models() {
    assert_eq!(
      resolve_model_id("openrouter", "openai/gpt-4o-mini"),
      "openrouter/openai/gpt-4o-mini".to_string()
    );
    assert_eq!(
      resolve_model_id("openrouter", "openrouter/openai/gpt-4o-mini"),
      "openrouter/openai/gpt-4o-mini".to_string()
    );
    assert_eq!(
      resolve_model_id("openai", "gpt-4o-mini"),
      "openai/gpt-4o-mini".to_string()
    );
    assert_eq!(
      resolve_model_id("", "openrouter/openai/gpt-4o-mini"),
      "openrouter/openai/gpt-4o-mini".to_string()
    );
  }

  #[tokio::test]
  async fn test_openrouter_nested_model_id_routes_to_openrouter_provider() {
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "openrouter".to_string();
    config.models.model = "openai/gpt-4o-mini".to_string();

    let cokra = Cokra::new_with_model_client(config, build_openrouter_mock_client().await)
      .await
      .expect("create cokra");

    let result = cokra
      .run_turn("check openrouter model routing".to_string())
      .await
      .expect("run turn");

    assert_eq!(
      result.final_message,
      "openrouter nested model ok".to_string()
    );
    assert!(result.success);
  }

  #[tokio::test]
  async fn test_run_turn_tool_call_loop() {
    let tmp_path = std::env::temp_dir().join(format!("cokra-tool-loop-{}.txt", Uuid::new_v4()));
    std::fs::write(&tmp_path, "hello from tool loop").expect("write temp fixture");

    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mocktool".to_string();
    config.models.model = "mocktool/default".to_string();
    config.approval.policy = ApprovalMode::Auto;

    let cokra = Cokra::new_with_model_client(
      config,
      build_tool_loop_client(tmp_path.display().to_string()).await,
    )
    .await
    .expect("create cokra");

    let result = cokra
      .run_turn("read the file".to_string())
      .await
      .expect("run turn");

    assert_eq!(result.final_message, "tool loop complete");
    let _ = std::fs::remove_file(tmp_path);
  }

  #[tokio::test]
  async fn test_spawn_agent_respects_max_threads_limit() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mock".to_string();
    config.models.model = "mock/default".to_string();
    config.approval.policy = ApprovalMode::Auto;
    config.agents.max_threads = 1;
    config.cwd = tmpdir.path().to_path_buf();

    let cokra = Cokra::new_with_model_client(config, build_mock_client().await)
      .await
      .expect("create cokra");

    let first = cokra
      .agent_control
      .spawn_agent(
        "inspect repository".to_string(),
        None,
        Some("explorer".to_string()),
        cokra.thread_id().cloned(),
        1,
        Some(1),
      )
      .await
      .expect("first spawn should succeed");
    assert!(!first.to_string().is_empty());
    assert_eq!(cokra.list_thread_ids().len(), 2);

    let second = cokra
      .agent_control
      .spawn_agent(
        "inspect again".to_string(),
        None,
        Some("explorer".to_string()),
        cokra.thread_id().cloned(),
        1,
        Some(1),
      )
      .await;

    assert!(second.is_err());
  }

  #[tokio::test]
  async fn test_spawn_agent_wait_and_close_round_trip() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mock".to_string();
    config.models.model = "mock/default".to_string();
    config.approval.policy = ApprovalMode::Auto;
    config.cwd = tmpdir.path().to_path_buf();

    let cokra = Cokra::new_with_model_client(config, build_mock_client().await)
      .await
      .expect("create cokra");

    let spawn = cokra
      .execute_tool(crate::model::ToolCall {
        id: "spawn-1".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "spawn_agent".to_string(),
          arguments: serde_json::json!({
            "task": "Analyze the current repository"
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("spawn tool should succeed");

    let spawned: serde_json::Value =
      serde_json::from_str(&spawn.text_content()).expect("spawn json");
    let agent_id = spawned
      .get("agent_id")
      .and_then(serde_json::Value::as_str)
      .expect("agent id")
      .to_string();

    let wait = cokra
      .execute_tool(crate::model::ToolCall {
        id: "wait-1".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "wait".to_string(),
          arguments: serde_json::json!({
            "agent_ids": [agent_id.clone()],
            "timeout_ms": 10000
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("wait tool should succeed");

    let statuses: serde_json::Value =
      serde_json::from_str(&wait.text_content()).expect("wait json");
    assert_eq!(statuses[&agent_id]["Completed"], "mock reply");

    let close = cokra
      .execute_tool(crate::model::ToolCall {
        id: "close-1".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "close_agent".to_string(),
          arguments: serde_json::json!({
            "agent_id": agent_id
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("close tool should succeed");

    let closed: serde_json::Value =
      serde_json::from_str(&close.text_content()).expect("close json");
    assert_eq!(closed["status"], "Shutdown");
  }

  #[tokio::test]
  async fn test_team_mailbox_and_task_board_round_trip() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mock".to_string();
    config.models.model = "mock/default".to_string();
    config.approval.policy = ApprovalMode::Auto;
    config.cwd = tmpdir.path().to_path_buf();

    let cokra = Cokra::new_with_model_client(config, build_mock_client().await)
      .await
      .expect("create cokra");

    let spawn = cokra
      .execute_tool(crate::model::ToolCall {
        id: "spawn-team-1".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "spawn_agent".to_string(),
          arguments: serde_json::json!({
            "task": "You are teammate alex. Wait for team instructions."
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("spawn tool should succeed");
    let spawned: serde_json::Value =
      serde_json::from_str(&spawn.text_content()).expect("spawn json");
    let agent_id = spawned["agent_id"].as_str().expect("agent id").to_string();

    let _ = cokra
      .execute_tool(crate::model::ToolCall {
        id: "wait-team-1".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "wait".to_string(),
          arguments: serde_json::json!({
            "agent_ids": [agent_id.clone()],
            "timeout_ms": 10000
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("wait should succeed");

    let created = cokra
      .execute_tool(crate::model::ToolCall {
        id: "task-create-1".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "create_team_task".to_string(),
          arguments: serde_json::json!({
            "title": "Inspect current project structure",
            "details": "Review core modules and summarize architecture.",
            "assignee_thread_id": agent_id.clone()
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("create task should succeed");
    let task: TeamTask = serde_json::from_str(&created.text_content()).expect("task json");
    assert_eq!(task.status, TeamTaskStatus::Pending);
    assert_eq!(task.assignee_thread_id.as_deref(), Some(agent_id.as_str()));

    let _ = cokra
      .execute_tool(crate::model::ToolCall {
        id: "mail-send-1".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "send_team_message".to_string(),
          arguments: serde_json::json!({
            "recipient_thread_id": agent_id.clone(),
            "message": "Please inspect the repository structure and update your task when done."
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("send team message should succeed");

    let read = execute_tool_as_thread(
      &cokra,
      agent_id.clone(),
      "read_team_messages",
      serde_json::json!({"unread_only": true}),
    )
    .await;
    let messages: Vec<cokra_protocol::TeamMessage> =
      serde_json::from_str(&read.text_content()).expect("messages json");
    assert_eq!(messages.len(), 1);
    assert!(messages[0].unread);

    let updated = execute_tool_as_thread(
      &cokra,
      agent_id.clone(),
      "update_team_task",
      serde_json::json!({
        "task_id": task.id,
        "status": "Completed",
        "note": "Finished repository structure review."
      }),
    )
    .await;
    let updated_task: TeamTask =
      serde_json::from_str(&updated.text_content()).expect("updated task json");
    assert_eq!(updated_task.status, TeamTaskStatus::Completed);

    let status = cokra
      .execute_tool(crate::model::ToolCall {
        id: "team-status-1".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "team_status".to_string(),
          arguments: "{}".to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("team status should succeed");
    let snapshot: TeamSnapshot =
      serde_json::from_str(&status.text_content()).expect("snapshot json");
    assert!(
      snapshot
        .members
        .iter()
        .any(|member| member.thread_id == agent_id)
    );
    assert!(
      snapshot
        .tasks
        .iter()
        .any(|item| item.status == TeamTaskStatus::Completed)
    );
    assert_eq!(snapshot.unread_counts.get(&agent_id), Some(&0usize));
  }

  #[tokio::test]
  async fn test_team_state_restores_after_restart() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mock".to_string();
    config.models.model = "mock/default".to_string();
    config.approval.policy = ApprovalMode::Auto;
    config.cwd = tmpdir.path().to_path_buf();

    let cokra = Cokra::new_with_model_client(config.clone(), build_mock_client().await)
      .await
      .expect("create cokra");

    let created = cokra
      .execute_tool(crate::model::ToolCall {
        id: "persist-task-1".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "create_team_task".to_string(),
          arguments: serde_json::json!({
            "title": "Persistent task",
            "details": "Should survive restart"
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("create task");
    let task: TeamTask = serde_json::from_str(&created.text_content()).expect("task json");
    assert_eq!(task.title, "Persistent task");

    let _ = cokra
      .execute_tool(crate::model::ToolCall {
        id: "persist-mail-1".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "send_team_message".to_string(),
          arguments: serde_json::json!({
            "message": "Persistent mail"
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("send message");

    cokra.shutdown().await.expect("shutdown first runtime");

    let restored = Cokra::new_with_model_client(config, build_mock_client().await)
      .await
      .expect("recreate cokra");
    let status = restored
      .execute_tool(crate::model::ToolCall {
        id: "persist-status-1".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "team_status".to_string(),
          arguments: "{}".to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("team status");
    let snapshot: TeamSnapshot =
      serde_json::from_str(&status.text_content()).expect("snapshot json");
    assert!(
      snapshot
        .tasks
        .iter()
        .any(|item| item.title == "Persistent task")
    );
    let current_root = restored.thread_id().expect("thread id").to_string();
    // Root thread authored the message; unread counts should remain stable across restarts.
    assert_eq!(snapshot.unread_counts.get(&current_root), Some(&0usize));
  }

  #[tokio::test]
  async fn test_cleanup_team_clears_persisted_state() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mock".to_string();
    config.models.model = "mock/default".to_string();
    config.approval.policy = ApprovalMode::Auto;
    config.cwd = tmpdir.path().to_path_buf();

    let cokra = Cokra::new_with_model_client(config.clone(), build_mock_client().await)
      .await
      .expect("create cokra");
    let _ = cokra
      .execute_tool(crate::model::ToolCall {
        id: "cleanup-task-1".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "create_team_task".to_string(),
          arguments: serde_json::json!({
            "title": "Temporary task"
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("create temp task");
    let _ = cokra
      .execute_tool(crate::model::ToolCall {
        id: "cleanup-team-1".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "cleanup_team".to_string(),
          arguments: "{}".to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("cleanup team");
    cokra.shutdown().await.expect("shutdown runtime");

    let restored = Cokra::new_with_model_client(config, build_mock_client().await)
      .await
      .expect("recreate cokra");
    let status = restored
      .execute_tool(crate::model::ToolCall {
        id: "cleanup-status-1".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "team_status".to_string(),
          arguments: "{}".to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("team status");
    let snapshot: TeamSnapshot =
      serde_json::from_str(&status.text_content()).expect("snapshot json");
    assert!(snapshot.tasks.is_empty());
    let unread_total: usize = snapshot.unread_counts.values().copied().sum();
    assert_eq!(unread_total, 0);
  }

  #[tokio::test]
  async fn test_drop_clears_team_runtime_registration() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mock".to_string();
    config.models.model = "mock/default".to_string();
    config.approval.policy = ApprovalMode::Auto;
    config.cwd = tmpdir.path().to_path_buf();

    let cokra = Cokra::new_with_model_client(config, build_mock_client().await)
      .await
      .expect("create cokra");
    let thread_id = cokra
      .thread_id()
      .cloned()
      .expect("root thread id should exist");
    assert!(runtime_for_thread(&thread_id.to_string()).is_some());

    drop(cokra);

    assert!(runtime_for_thread(&thread_id.to_string()).is_none());
  }

  #[tokio::test]
  async fn test_claim_team_task_marks_in_progress_for_current_thread() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mock".to_string();
    config.models.model = "mock/default".to_string();
    config.approval.policy = ApprovalMode::Auto;
    config.cwd = tmpdir.path().to_path_buf();

    let cokra = Cokra::new_with_model_client(config, build_mock_client().await)
      .await
      .expect("create cokra");

    let created = cokra
      .execute_tool(crate::model::ToolCall {
        id: "claim-task-create".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "create_team_task".to_string(),
          arguments: serde_json::json!({
            "title": "Claim me"
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("create task");
    let task: TeamTask = serde_json::from_str(&created.text_content()).expect("task json");

    let claimed = cokra
      .execute_tool(crate::model::ToolCall {
        id: "claim-task-run".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "claim_team_task".to_string(),
          arguments: serde_json::json!({
            "task_id": task.id,
            "note": "Starting work now"
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("claim task");
    let claimed_task: TeamTask =
      serde_json::from_str(&claimed.text_content()).expect("claimed json");
    assert_eq!(claimed_task.status, TeamTaskStatus::InProgress);
    assert_eq!(
      claimed_task.assignee_thread_id,
      cokra.thread_id().map(ToString::to_string)
    );
    assert!(
      claimed_task
        .notes
        .iter()
        .any(|note| note == "Starting work now")
    );
  }

  #[tokio::test]
  async fn test_team_plan_gate_blocks_mutation_until_approved() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mock".to_string();
    config.models.model = "mock/default".to_string();
    config.approval.policy = ApprovalMode::Auto;
    config.cwd = tmpdir.path().to_path_buf();

    let cokra = Cokra::new_with_model_client(config, build_mock_client().await)
      .await
      .expect("create cokra");

    let spawn = cokra
      .execute_tool(crate::model::ToolCall {
        id: "plan-gate-spawn".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "spawn_agent".to_string(),
          arguments: serde_json::json!({
            "task": "You are teammate leex. Wait for plan review."
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("spawn");
    let spawned: serde_json::Value =
      serde_json::from_str(&spawn.text_content()).expect("spawn json");
    let agent_id = spawned["agent_id"].as_str().expect("agent id").to_string();
    let _ = cokra
      .execute_tool(crate::model::ToolCall {
        id: "plan-gate-wait".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "wait".to_string(),
          arguments: serde_json::json!({
            "agent_ids": [agent_id.clone()],
            "timeout_ms": 10000
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("wait");

    let submitted = execute_tool_as_thread(
      &cokra,
      agent_id.clone(),
      "submit_team_plan",
      serde_json::json!({
        "summary": "Refactor module safely",
        "steps": ["Inspect files", "Edit target module", "Run focused tests"],
        "requires_approval": true
      }),
    )
    .await;
    let plan: cokra_protocol::TeamPlan =
      serde_json::from_str(&submitted.text_content()).expect("plan json");

    let target = tmpdir.path().join("plan-gated.txt");
    let blocked = execute_tool_as_thread_result(
      &cokra,
      agent_id.clone(),
      "write_file",
      serde_json::json!({
        "file_path": target.display().to_string(),
        "content": "blocked until approved"
      }),
    )
    .await;
    let err = blocked.expect_err("mutation should be blocked before approval");
    assert!(err.to_string().contains("plan approval required"));

    let _ = cokra
      .execute_tool(crate::model::ToolCall {
        id: "plan-gate-approve".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "approve_team_plan".to_string(),
          arguments: serde_json::json!({
            "plan_id": plan.id,
            "approved": true,
            "note": "Proceed"
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("approve plan");

    let write = execute_tool_as_thread_result(
      &cokra,
      agent_id,
      "write_file",
      serde_json::json!({
        "file_path": target.display().to_string(),
        "content": "approved now"
      }),
    )
    .await;
    assert!(write.is_ok(), "mutation should succeed after approval");
  }

  #[tokio::test]
  async fn test_team_channel_and_queue_mailbox_round_trip() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mock".to_string();
    config.models.model = "mock/default".to_string();
    config.approval.policy = ApprovalMode::Auto;
    config.cwd = tmpdir.path().to_path_buf();

    let cokra = Cokra::new_with_model_client(config, build_mock_client().await)
      .await
      .expect("create cokra");

    let spawn = cokra
      .execute_tool(crate::model::ToolCall {
        id: "queue-spawn".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "spawn_agent".to_string(),
          arguments: serde_json::json!({"task": "You are teammate alex."}).to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("spawn");
    let spawned: serde_json::Value =
      serde_json::from_str(&spawn.text_content()).expect("spawn json");
    let agent_id = spawned["agent_id"].as_str().expect("agent id").to_string();
    let _ = cokra
      .execute_tool(crate::model::ToolCall {
        id: "queue-wait".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "wait".to_string(),
          arguments: serde_json::json!({"agent_ids": [agent_id.clone()], "timeout_ms": 10000})
            .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("wait");

    let _ = cokra
      .execute_tool(crate::model::ToolCall {
        id: "channel-send".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "send_team_message".to_string(),
          arguments: serde_json::json!({
            "channel": "research",
            "message": "Shared finding"
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("channel send");

    let channel_messages = execute_tool_as_thread(
      &cokra,
      agent_id.clone(),
      "read_team_messages",
      serde_json::json!({"unread_only": true}),
    )
    .await;
    let channel_messages: Vec<cokra_protocol::TeamMessage> =
      serde_json::from_str(&channel_messages.text_content()).expect("channel messages json");
    assert!(channel_messages.iter().any(|message| {
      message.kind == cokra_protocol::TeamMessageKind::Channel
        && message.route_key.as_deref() == Some("research")
    }));

    let _ = cokra
      .execute_tool(crate::model::ToolCall {
        id: "queue-send".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "send_team_message".to_string(),
          arguments: serde_json::json!({
            "queue_name": "review",
            "message": "Take review item #1"
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("queue send");

    let claimed = execute_tool_as_thread(
      &cokra,
      agent_id,
      "claim_team_messages",
      serde_json::json!({"queue_name": "review", "limit": 1}),
    )
    .await;
    let claimed: Vec<cokra_protocol::TeamMessage> =
      serde_json::from_str(&claimed.text_content()).expect("claimed json");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].kind, cokra_protocol::TeamMessageKind::Queue);
    assert_eq!(claimed[0].route_key.as_deref(), Some("review"));
    assert!(claimed[0].claimed_by_thread_id.is_some());
  }

  #[tokio::test]
  async fn test_team_task_handoff_and_claim_next_workflow() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mock".to_string();
    config.models.model = "mock/default".to_string();
    config.approval.policy = ApprovalMode::Auto;
    config.cwd = tmpdir.path().to_path_buf();

    let cokra = Cokra::new_with_model_client(config, build_mock_client().await)
      .await
      .expect("create cokra");

    let spawn = cokra
      .execute_tool(crate::model::ToolCall {
        id: "workflow-spawn".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "spawn_agent".to_string(),
          arguments: serde_json::json!({"task": "You are teammate alex."}).to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("spawn");
    let spawned: serde_json::Value =
      serde_json::from_str(&spawn.text_content()).expect("spawn json");
    let alex_id = spawned["agent_id"].as_str().expect("agent id").to_string();
    let _ = cokra
      .execute_tool(crate::model::ToolCall {
        id: "workflow-wait".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "wait".to_string(),
          arguments: serde_json::json!({"agent_ids": [alex_id.clone()], "timeout_ms": 10000})
            .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("wait");

    let created = cokra
      .execute_tool(crate::model::ToolCall {
        id: "workflow-create".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "create_team_task".to_string(),
          arguments: serde_json::json!({"title": "Implement feature"}).to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("create");
    let task: TeamTask = serde_json::from_str(&created.text_content()).expect("task json");

    let _ = cokra
      .execute_tool(crate::model::ToolCall {
        id: "workflow-assign".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "assign_team_task".to_string(),
          arguments: serde_json::json!({
            "task_id": task.id,
            "assignee_thread_id": cokra.thread_id().map(ToString::to_string).expect("thread")
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("assign");

    let handed = cokra
      .execute_tool(crate::model::ToolCall {
        id: "workflow-handoff".to_string(),
        call_type: "function".to_string(),
        function: crate::model::ToolCallFunction {
          name: "handoff_team_task".to_string(),
          arguments: serde_json::json!({
            "task_id": task.id,
            "to_thread_id": alex_id.clone(),
            "review_mode": true,
            "note": "Ready for review"
          })
          .to_string(),
        },
        provider_meta: None,
      })
      .await
      .expect("handoff");
    let handed_task: TeamTask = serde_json::from_str(&handed.text_content()).expect("handoff json");
    assert_eq!(handed_task.status, TeamTaskStatus::Review);
    assert_eq!(
      handed_task.assignee_thread_id.as_deref(),
      Some(alex_id.as_str())
    );

    let claimed = execute_tool_as_thread(
      &cokra,
      alex_id,
      "claim_next_team_task",
      serde_json::json!({}),
    )
    .await;
    let claimed_task: TeamTask =
      serde_json::from_str(&claimed.text_content()).expect("claimed json");
    assert_eq!(claimed_task.status, TeamTaskStatus::InProgress);
    assert_eq!(claimed_task.title, "Implement feature");
  }

  #[tokio::test]
  async fn test_provider_connect_catalog_marks_api_key_provider_connected() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mock".to_string();
    config.models.model = "mock/default".to_string();
    config.approval.policy = ApprovalMode::Auto;
    config.cwd = tmpdir.path().to_path_buf();

    // Avoid HOME-scoped file storage in tests: use a shared in-memory credential
    // store so the registry and test AuthManager see consistent state.
    let storage = std::sync::Arc::new(crate::model::auth_store::MemoryCredentialStorage::new());
    let auth = std::sync::Arc::new(
      crate::model::auth::AuthManager::with_storage(storage).expect("auth manager"),
    );
    let registry = std::sync::Arc::new(crate::model::ProviderRegistry::new_with_auth(Some(
      auth.clone(),
    )));
    registry.register(MockProvider::new()).await;
    registry
      .set_default("mock")
      .await
      .expect("set mock default");
    let model_client = std::sync::Arc::new(
      crate::model::ModelClient::new(registry)
        .await
        .expect("build model client"),
    );

    let cokra = Cokra::new_with_model_client(config, model_client)
      .await
      .expect("create cokra");

    // Reuse the TUI path's persistence semantics directly via auth manager.
    auth
      .save(
        "openrouter",
        crate::model::auth::Credentials::ApiKey {
          key: "test-openrouter-key".to_string(),
        },
      )
      .await
      .expect("save key");

    let after = cokra
      .model_client()
      .registry()
      .is_connect_catalog_provider_connected("openrouter")
      .await;
    assert!(after);
  }

  #[tokio::test]
  async fn test_exec_approval_round_trip_unblocks_turn() {
    let tmp_path = std::env::temp_dir().join(format!("cokra-approval-{}.txt", Uuid::new_v4()));

    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mockapproval".to_string();
    config.models.model = "mockapproval/default".to_string();
    config.approval.policy = ApprovalMode::Ask;

    let cokra = Cokra::new_with_model_client(
      config,
      build_approval_client(tmp_path.display().to_string()).await,
    )
    .await
    .expect("create cokra");

    let _ = cokra
      .submit(Op::UserInput {
        items: vec![UserInput::Text {
          text: "write with approval".to_string(),
          text_elements: Vec::new(),
        }],
        final_output_json_schema: None,
      })
      .await
      .expect("submit user input");

    let mut approval_id = None;
    let mut approval_turn_id = None;
    for _ in 0..40 {
      let evt = timeout(Duration::from_secs(2), cokra.next_event())
        .await
        .expect("next_event timeout")
        .expect("next_event");
      match evt.msg {
        EventMsg::ExecApprovalRequest(ev) => {
          approval_id = Some(ev.id);
          approval_turn_id = Some(ev.turn_id);
          break;
        }
        EventMsg::Error(err) => panic!("unexpected error: {}", err.user_facing_message),
        _ => {}
      }
    }

    let approval_id = approval_id.expect("expected exec approval request");
    let approval_turn_id = approval_turn_id.expect("expected approval turn id");

    let _ = cokra
      .submit(Op::ExecApproval {
        id: approval_id,
        turn_id: Some(approval_turn_id),
        decision: ReviewDecision::Approved,
      })
      .await
      .expect("submit approval response");

    let mut saw_complete = false;
    for _ in 0..40 {
      let evt = timeout(Duration::from_secs(2), cokra.next_event())
        .await
        .expect("next_event timeout")
        .expect("next_event");
      match evt.msg {
        EventMsg::TurnComplete(done) => {
          assert!(matches!(done.status, CompletionStatus::Success));
          saw_complete = true;
          break;
        }
        EventMsg::Error(err) => panic!("unexpected error: {}", err.user_facing_message),
        _ => {}
      }
    }

    assert!(saw_complete, "expected turn completion after approval");
    let written = std::fs::read_to_string(&tmp_path).expect("read written file");
    assert_eq!(written, "approved".to_string());
    let _ = std::fs::remove_file(tmp_path);
  }

  #[tokio::test]
  async fn test_request_user_input_round_trip_unblocks_turn() {
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mockinput".to_string();
    config.models.model = "mockinput/default".to_string();
    config.approval.policy = ApprovalMode::Auto;

    let cokra = Cokra::new_with_model_client(config, build_request_user_input_client().await)
      .await
      .expect("create cokra");

    let _ = cokra
      .submit(Op::UserInput {
        items: vec![UserInput::Text {
          text: "ask me".to_string(),
          text_elements: Vec::new(),
        }],
        final_output_json_schema: None,
      })
      .await
      .expect("submit user input");

    let mut started_turn_id = None;
    let mut request_turn_id = None;
    let mut request_call_id = None;
    for _ in 0..40 {
      let evt = timeout(Duration::from_secs(2), cokra.next_event())
        .await
        .expect("next_event timeout")
        .expect("next_event");
      match evt.msg {
        EventMsg::TurnStarted(ev) => {
          started_turn_id = Some(ev.turn_id);
        }
        EventMsg::RequestUserInput(ev) => {
          request_turn_id = Some(ev.turn_id);
          request_call_id = Some(ev.call_id);
          break;
        }
        EventMsg::Error(err) => panic!("unexpected error: {}", err.user_facing_message),
        _ => {}
      }
    }

    let started_turn_id = started_turn_id.expect("expected turn started event");
    let request_turn_id = request_turn_id.expect("expected request_user_input event");
    let request_call_id = request_call_id.expect("expected request call id");
    assert_eq!(started_turn_id, request_turn_id);
    assert_eq!(request_call_id, "call_input_1");

    let _ = cokra
      .submit(Op::UserInputAnswer {
        id: request_turn_id,
        response: cokra_protocol::user_input::RequestUserInputResponse {
          answers: std::collections::HashMap::from([(
            "name".to_string(),
            cokra_protocol::RequestUserInputAnswer {
              answers: vec!["leex".to_string()],
            },
          )]),
        },
      })
      .await
      .expect("submit user input answer");

    let mut saw_complete = false;
    for _ in 0..40 {
      let evt = timeout(Duration::from_secs(2), cokra.next_event())
        .await
        .expect("next_event timeout")
        .expect("next_event");
      match evt.msg {
        EventMsg::TurnComplete(done) => {
          assert!(matches!(done.status, CompletionStatus::Success));
          saw_complete = true;
          break;
        }
        EventMsg::Error(err) => panic!("unexpected error: {}", err.user_facing_message),
        _ => {}
      }
    }

    assert!(
      saw_complete,
      "expected turn completion after request_user_input"
    );
  }
}
