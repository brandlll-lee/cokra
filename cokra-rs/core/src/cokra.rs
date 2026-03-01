use std::collections::VecDeque;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use futures::Stream;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, watch};
use uuid::Uuid;

use cokra_config::{ApprovalMode, Config, SandboxMode};
use cokra_protocol::{
  AskForApproval, CompletionStatus, Event, EventMsg, Op, ReadOnlyAccess, SandboxPolicy,
  SessionConfiguredEvent, Submission, TurnAbortedEvent, UserInput as ProtocolUserInput,
};

use crate::agent::{AgentControl, AgentStatus, Turn};
use crate::model::{ChatResponse, ModelClient, ToolCall, Usage, init_model_layer};
use crate::session::Session;
use crate::thread_manager::ThreadManager;
use crate::tools::build_default_tools;
use crate::tools::context::{FunctionCallError, ToolContext, ToolOutput};
use crate::tools::handlers::spawn_agent::{
  clear_spawn_agent_runtime, configure_spawn_agent_runtime,
};
use crate::tools::registry::ToolRegistry;
use crate::tools::router::ToolRouter;
use crate::turn::TurnConfig;

pub(crate) const SUBMISSION_CHANNEL_CAPACITY: usize = 64;

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

  pub(crate) model_client: Arc<ModelClient>,
  pub(crate) tool_context: Arc<ToolContext>,
  pub(crate) config: Arc<Config>,
  pub(crate) turn_state: Arc<RwLock<TurnState>>,
  pub(crate) event_bus: Arc<broadcast::Sender<EventMsg>>,
  pub(crate) tool_registry: Arc<ToolRegistry>,
  pub(crate) tool_router: Arc<ToolRouter>,
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

    let session = Arc::new(Session::new());
    let root_thread_id = session.thread_id().cloned().unwrap_or_default();
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

    let (tool_registry, tool_router) = build_default_tools(&config);
    let agent_control = Arc::new(AgentControl::new(
      Uuid::new_v4().to_string(),
      model_client.clone(),
      tool_registry.clone(),
      session.clone(),
      turn_config,
      tx_raw_event,
      thread_manager.downgrade_state(),
      guards,
      root_thread_id.clone(),
    ));
    configure_spawn_agent_runtime(
      agent_control.clone(),
      root_thread_id,
      Some(config.agents.max_threads),
      1,
    );
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
      tool_context: Arc::new(ToolContext::default()),
      config,
      turn_state: Arc::new(RwLock::new(TurnState::default())),
      event_bus,
      tool_registry,
      tool_router,
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

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
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
          if !item.result.is_empty() {
            final_message = item.result;
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
      .map_err(|e| FunctionCallError::InvalidArguments(format!("invalid tool args: {e}")))?;

    self
      .tool_router
      .route_tool_call(&tool_call.function.name, args)
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

  pub fn list_thread_ids(&self) -> Vec<cokra_protocol::ThreadId> {
    self.thread_manager.list_thread_ids()
  }

  pub async fn shutdown(self) -> anyhow::Result<()> {
    let _ = self.submit(Op::Shutdown).await?;
    self.agent_control.stop().await?;
    self.session.shutdown().await?;
    clear_spawn_agent_runtime();
    Ok(())
  }
}

fn build_turn_config(config: &Config) -> TurnConfig {
  let provider = config.models.provider.trim();
  let model = config.models.model.trim();

  let resolved_model = if model.contains('/') || provider.is_empty() {
    model.to_string()
  } else {
    format!("{provider}/{model}")
  };

  TurnConfig {
    model: resolved_model,
    ..TurnConfig::default()
  }
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
      writable_roots: vec![
        std::env::current_dir()
          .unwrap_or_else(|_| PathBuf::from("."))
          .display()
          .to_string(),
      ],
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
  let _ = event_bus.send(msg.clone());
  let _ = tx_event
    .send(Event {
      id: Uuid::new_v4().to_string(),
      msg,
    })
    .await;
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
        cwd: _,
        approval_policy,
        sandbox_policy,
        model,
      } => {
        turn_config.model = model.clone();
        agent_control.set_turn_config(turn_config.clone()).await;
        emit_event(
          &tx_event,
          &event_bus,
          EventMsg::SessionConfigured(SessionConfiguredEvent {
            thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
            model,
            approval_policy: format!("{approval_policy:?}"),
            sandbox_mode: format!("{sandbox_policy:?}"),
          }),
        )
        .await;
      }
      Op::UserInput { items, .. } => {
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

  let mut fut = Box::pin(agent_control.process_turn(Turn { user_message }));
  loop {
    tokio::select! {
      res = &mut fut => {
        if let Err(err) = res {
          emit_event(
            tx_event,
            event_bus,
            EventMsg::Error(cokra_protocol::ErrorEvent {
              thread_id: session.thread_id().cloned().unwrap_or_default().to_string(),
              turn_id: turn_id.to_string(),
              error: err.to_string(),
              user_facing_message: err.to_string(),
              details: format!("{err:?}"),
            }),
          ).await;
        }
        break;
      }
      maybe_sub = rx_sub.recv() => {
        let Some(next_sub) = maybe_sub else {
          break;
        };
        match next_sub.op {
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
            break;
          }
          Op::Shutdown => {
            emit_event(tx_event, event_bus, EventMsg::ShutdownComplete).await;
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
  use std::sync::{Arc, Mutex};

  use async_trait::async_trait;
  use futures::Stream;
  use reqwest::Client;
  use uuid::Uuid;

  use super::Cokra;
  use crate::model::provider::ModelProvider;
  use crate::model::{
    ChatRequest, ChatResponse, Choice, ChoiceMessage, Chunk, ContentDelta, ListModelsResponse,
    Message, ModelClient, ModelInfo, ProviderConfig, ProviderRegistry, ToolCall,
    ToolCallDelta, ToolCallFunction, Usage,
  };
  use cokra_config::ApprovalMode;
  use cokra_protocol::{EventMsg, Op, UserInput};

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
    let mut saw_started = false;
    let mut saw_item_started = false;
    let mut saw_item_completed = false;
    let mut saw_complete = false;

    for _ in 0..20 {
      let evt = cokra.next_event().await.expect("next event");
      match evt.msg {
        EventMsg::SessionConfigured(_) => saw_configured = true,
        EventMsg::TurnStarted(_) => saw_started = true,
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
    assert!(saw_started);
    assert!(saw_item_started);
    assert!(saw_item_completed);
    assert!(saw_complete);
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
    let mut config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    config.models.provider = "mock".to_string();
    config.models.model = "mock/default".to_string();
    config.approval.policy = ApprovalMode::Auto;
    config.agents.max_threads = 1;

    let cokra = Cokra::new_with_model_client(config, build_mock_client().await)
      .await
      .expect("create cokra");

    let first = cokra
      .agent_control
      .spawn_agent(
        "inspect repository".to_string(),
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
        Some("explorer".to_string()),
        cokra.thread_id().cloned(),
        1,
        Some(1),
      )
      .await;

    assert!(second.is_err());
  }
}
