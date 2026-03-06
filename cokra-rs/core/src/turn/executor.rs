//! Turn Executor
//!
//! Executes a turn (one user interaction cycle) in a Cokra session.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::model::Message as ModelMessage;
use crate::model::ModelClient;
use crate::session::Session;
use crate::tools::registry::ToolRegistry;
use crate::tools::router::ToolRouter;
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
  pub tool_output_truncation: TruncationPolicy,
  pub context_window_limit: Option<usize>,
  pub auto_compact_token_limit: Option<usize>,
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
      tool_output_truncation: TruncationPolicy::Tokens(DEFAULT_TOOL_OUTPUT_TOKENS),
      context_window_limit: Some(128_000),
      auto_compact_token_limit: Some(96_000),
    }
  }
}

#[derive(Clone)]
pub struct TurnExecutor {
  model_client: Arc<ModelClient>,
  tool_registry: Arc<ToolRegistry>,
  tool_router: Arc<ToolRouter>,
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
      session,
      tx_event,
      config,
      cancellation_token: CancellationToken::new(),
    }
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

    let messages = self.build_messages(input.clone()).await?;

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
    );

    let output = match sse_executor
      .run_sse_interaction(messages, thread_id.clone(), turn_id.clone())
      .await
    {
      Ok(output) => output,
      Err(e) => {
        self
          .send_event(EventMsg::Error(ErrorEvent {
            thread_id: thread_id.clone(),
            turn_id: turn_id.clone(),
            error: e.to_string(),
            user_facing_message: e.to_string(),
            details: format!("{e:?}"),
          }))
          .await?;
        return Err(e);
      }
    };

    self
      .send_event(EventMsg::TurnComplete(TurnCompleteEvent {
        thread_id,
        turn_id,
        status: CompletionStatus::Success,
        end_time: chrono::Utc::now().timestamp(),
      }))
      .await?;

    Ok(output)
  }

  async fn build_messages(&self, input: UserInput) -> Result<Vec<ModelMessage>, TurnError> {
    let mut messages = Vec::new();

    if let Some(system) = &self.config.system_prompt {
      messages.push(ModelMessage::System(system.clone()));
    }

    // 1:1 codex: inject environment_context so the model knows the cwd
    // and uses absolute paths for file tools (read_file, write_file, list_dir).
    let env_context = format!(
      "<environment_context>\n  <cwd>{}</cwd>\n</environment_context>",
      self.config.cwd.display()
    );
    messages.push(ModelMessage::User(env_context));

    let history = if let Some(limit) = self.config.context_window_limit {
      self.session.get_history_for_prompt(limit).await
    } else {
      self.session.get_history(100).await
    };
    messages.extend(history);
    messages.push(ModelMessage::User(input.content));

    Ok(messages)
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

#[cfg(test)]
mod tests {
  use std::pin::Pin;
  use std::sync::Arc;

  use async_trait::async_trait;
  use futures::Stream;
  use reqwest::Client;
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
  use crate::tools::registry::ToolRegistry;
  use crate::tools::router::ToolRouter;
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
}
