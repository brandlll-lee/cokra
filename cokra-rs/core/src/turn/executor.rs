//! Turn Executor
//!
//! Executes a turn (one user interaction cycle) in a Cokra session.

use std::sync::Arc;

use tokio::sync::mpsc;
use uuid::Uuid;

use crate::model::{Message as ModelMessage, ModelClient};
use crate::session::Session;
use crate::tools::registry::ToolRegistry;
use cokra_protocol::{
  CompletionStatus, ErrorEvent, EventMsg, ModeKind, TurnCompleteEvent, TurnStartedEvent,
};

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
}

impl Default for TurnConfig {
  fn default() -> Self {
    Self {
      model: "gpt-4o".to_string(),
      temperature: Some(0.2),
      max_tokens: Some(4096),
      system_prompt: None,
      enable_tools: true,
    }
  }
}

#[derive(Clone)]
pub struct TurnExecutor {
  model_client: Arc<ModelClient>,
  tool_registry: Arc<ToolRegistry>,
  session: Arc<Session>,
  tx_event: mpsc::Sender<Event>,
  config: TurnConfig,
}

impl TurnExecutor {
  pub fn new(
    model_client: Arc<ModelClient>,
    tool_registry: Arc<ToolRegistry>,
    session: Arc<Session>,
    tx_event: mpsc::Sender<Event>,
    config: TurnConfig,
  ) -> Self {
    Self {
      model_client,
      tool_registry,
      session,
      tx_event,
      config,
    }
  }

  pub async fn run_turn(&self, input: UserInput) -> Result<TurnResult, TurnError> {
    let thread_id = self
      .session
      .id()
      .unwrap_or_else(|| Uuid::new_v4().to_string());
    let turn_id = Uuid::new_v4().to_string();

    self
      .send_event(EventMsg::TurnStarted(TurnStartedEvent {
        thread_id: thread_id.clone(),
        turn_id: turn_id.clone(),
        mode: ModeKind::Default,
        model: self.config.model.clone(),
        start_time: chrono::Utc::now().timestamp(),
      }))
      .await?;

    let messages = self.build_messages(input.clone()).await?;
    let sse_executor = SseTurnExecutor::new(
      self.model_client.clone(),
      self.tool_registry.clone(),
      self.session.clone(),
      self.tx_event.clone(),
      self.config.clone(),
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

    let history = self.session.get_history(100).await;
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
  use tokio::sync::{Mutex, mpsc};

  use cokra_protocol::{ContentDeltaEvent, EventMsg, ResponseEvent};

  use super::{TurnConfig, TurnExecutor, UserInput};
  use crate::model::provider::ModelProvider;
  use crate::model::{
    ChatRequest, ChatResponse, Chunk, ListModelsResponse, ModelClient, ModelError, ModelInfo,
    ProviderConfig, ProviderRegistry,
  };
  use crate::session::Session;
  use crate::tools::registry::ToolRegistry;

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
    }
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
    let session = Arc::new(Session::new());
    let (tx_event, mut rx_event) = mpsc::channel(64);

    let executor = TurnExecutor::new(
      model_client,
      tool_registry,
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
    while let Ok(event) = rx_event.try_recv() {
      match event {
        EventMsg::TurnStarted(_) => labels.push("turn_started"),
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
  }
}
