use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use futures::Stream;
use tokio::sync::{RwLock, broadcast, mpsc, watch};

use cokra_config::Config;
use cokra_protocol::{EventMsg, Op};

use crate::agent::{AgentControl, AgentStatus, Turn};
use crate::model::{ChatResponse, ModelClient, ToolCall, Usage, init_model_layer};
use crate::session::Session;
use crate::tools::build_default_tools;
use crate::tools::context::{FunctionCallError, ToolContext, ToolOutput};
use crate::tools::registry::ToolRegistry;
use crate::tools::router::ToolRouter;
use crate::turn::TurnConfig;

/// Submission types for the core queue.
pub enum Submission {
  Op(OpSubmission),
  Shutdown,
}

/// Operation submission wrapper.
pub struct OpSubmission {
  pub sub_id: String,
  pub op: Op,
}

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
pub struct Cokra {
  pub(crate) tx_sub: mpsc::Sender<Submission>,
  pub(crate) rx_event: mpsc::Receiver<EventMsg>,
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
}

pub struct CokraSpawnOk {
  pub cokra: Cokra,
  pub thread_id: String,
}

impl Cokra {
  pub async fn new(config: Config) -> anyhow::Result<Self> {
    let model_client = init_model_layer(&config)
      .await
      .context("failed to initialize model layer")?;
    Self::new_with_model_client(config, model_client).await
  }

  pub async fn new_with_model_client(
    config: Config,
    model_client: Arc<ModelClient>,
  ) -> anyhow::Result<Self> {
    let config = Arc::new(config);

    let (tx_sub, _rx_sub) = mpsc::channel(128);
    let (tx_event, rx_event) = mpsc::channel(256);

    let session = Arc::new(Session::new());
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
      uuid::Uuid::new_v4().to_string(),
      model_client.clone(),
      tool_registry.clone(),
      session.clone(),
      turn_config,
      tx_event,
    ));
    agent_control.start().await?;

    let agent_status = agent_control.subscribe_status();
    let (event_bus, _event_rx) = broadcast::channel(512);

    Ok(Self {
      tx_sub,
      rx_event,
      agent_status,
      session,
      model_client,
      tool_context: Arc::new(ToolContext::default()),
      config,
      turn_state: Arc::new(RwLock::new(TurnState::default())),
      event_bus: Arc::new(event_bus),
      tool_registry,
      tool_router,
      agent_control,
    })
  }

  pub async fn submit(&self, op: Op) -> anyhow::Result<()> {
    self
      .tx_sub
      .send(Submission::Op(OpSubmission {
        sub_id: uuid::Uuid::new_v4().to_string(),
        op,
      }))
      .await?;
    Ok(())
  }

  pub async fn run_turn(&self, user_message: String) -> anyhow::Result<TurnResult> {
    {
      let mut state = self.turn_state.write().await;
      state.current_input = Some(user_message.clone());
    }

    let result = self
      .agent_control
      .process_turn(Turn {
        user_message: user_message.clone(),
      })
      .await?;

    {
      let mut state = self.turn_state.write().await;
      state.turns_executed += 1;
      state.last_output = Some(result.content.clone());
      state.current_input = None;
    }

    Ok(TurnResult {
      final_message: result.content,
      usage: result.usage,
      success: result.success,
    })
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
    self.session.subscribe_events()
  }

  pub fn agent_status(&self) -> AgentStatus {
    self.agent_status.borrow().clone()
  }

  pub fn thread_id(&self) -> Option<&cokra_protocol::ThreadId> {
    self.session.thread_id()
  }

  pub async fn shutdown(self) -> anyhow::Result<()> {
    self.tx_sub.send(Submission::Shutdown).await?;
    self.agent_control.stop().await?;
    self.session.shutdown().await?;
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

impl CokraSpawnOk {
  pub fn thread_id(&self) -> Option<&cokra_protocol::ThreadId> {
    self.cokra.thread_id()
  }
}

#[cfg(test)]
mod tests {
  use std::pin::Pin;
  use std::sync::Arc;

  use async_trait::async_trait;
  use futures::Stream;
  use reqwest::Client;

  use super::Cokra;
  use crate::model::provider::ModelProvider;
  use crate::model::{
    ChatRequest, ChatResponse, Choice, ChoiceMessage, Chunk, ListModelsResponse, ModelClient,
    ModelInfo, ProviderConfig, ProviderRegistry, Usage,
  };

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
      Ok(Box::pin(futures::stream::empty()))
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

  #[tokio::test]
  async fn test_cokra_creation() {
    let config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    let cokra = Cokra::new_with_model_client(config, build_mock_client().await)
      .await
      .expect("create cokra");
    let status = cokra.agent_status();
    assert!(matches!(
      status,
      crate::agent::AgentStatus::Ready | crate::agent::AgentStatus::PendingInit
    ));
  }

  #[tokio::test]
  async fn test_run_turn_with_mock_model() {
    let config = cokra_config::ConfigLoader::default()
      .load_with_cli_overrides(vec![])
      .expect("load config");
    let cokra = Cokra::new_with_model_client(config, build_mock_client().await)
      .await
      .expect("create cokra");

    let result = cokra.run_turn("hello".to_string()).await.expect("run turn");
    assert_eq!(result.final_message, "mock reply".to_string());
    assert!(result.success);
  }
}
