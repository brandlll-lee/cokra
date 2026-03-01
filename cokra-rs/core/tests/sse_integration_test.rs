use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;
use tokio::sync::{Mutex, mpsc};

use cokra_core::model::{
  ChatRequest, ChatResponse, Chunk, ListModelsResponse, Message, ModelClient, ModelError,
  ModelInfo, ModelProvider, ProviderConfig, ProviderRegistry,
};
use cokra_core::session::Session;
use cokra_core::tools::context::{FunctionCallError, ToolInvocation, ToolOutput};
use cokra_core::tools::registry::{ToolHandler, ToolKind, ToolRegistry};
use cokra_core::turn::{TurnConfig, TurnExecutor, UserInput};

use cokra_protocol::{ContentDeltaEvent, EventMsg, FunctionCall, FunctionCallEvent, ResponseEvent};

#[derive(Debug)]
enum MockStep {
  Delta(&'static str),
  Call {
    id: &'static str,
    name: &'static str,
    arguments: &'static str,
  },
  End,
}

#[derive(Debug)]
struct MockResponsesProvider {
  client: Client,
  config: ProviderConfig,
  scripts: Arc<Mutex<Vec<Vec<MockStep>>>>,
  calls: Arc<Mutex<u32>>,
}

impl MockResponsesProvider {
  fn new(scripts: Vec<Vec<MockStep>>) -> Self {
    Self {
      client: Client::new(),
      config: ProviderConfig {
        provider_id: "mock-sse-int".to_string(),
        ..Default::default()
      },
      scripts: Arc::new(Mutex::new(scripts)),
      calls: Arc::new(Mutex::new(0)),
    }
  }
}

#[async_trait]
impl ModelProvider for MockResponsesProvider {
  fn provider_id(&self) -> &'static str {
    "mock-sse-int"
  }

  fn provider_name(&self) -> &'static str {
    "Mock SSE Integration Provider"
  }

  async fn chat_completion(
    &self,
    _request: ChatRequest,
  ) -> cokra_core::model::Result<ChatResponse> {
    Err(ModelError::InvalidRequest(
      "chat_completion is unused in this test provider".to_string(),
    ))
  }

  async fn chat_completion_stream(
    &self,
    _request: ChatRequest,
  ) -> cokra_core::model::Result<Pin<Box<dyn Stream<Item = cokra_core::model::Result<Chunk>> + Send>>>
  {
    Ok(Box::pin(futures::stream::empty()))
  }

  async fn responses_stream(
    &self,
    request: ChatRequest,
  ) -> cokra_core::model::Result<
    Pin<Box<dyn Stream<Item = cokra_core::model::Result<ResponseEvent>> + Send>>,
  > {
    let mut calls = self.calls.lock().await;
    *calls += 1;

    if *calls == 2 {
      let saw_tool_output = request.messages.iter().any(|msg| {
        matches!(msg, Message::Tool { tool_call_id, content } if tool_call_id == "read_1" && content.contains("hello from tool"))
      });
      if !saw_tool_output {
        return Err(ModelError::InvalidRequest(
          "missing function_call_output simulation".to_string(),
        ));
      }
    }

    let mut scripts = self.scripts.lock().await;
    if scripts.is_empty() {
      return Err(ModelError::InvalidRequest(
        "mock response script exhausted".to_string(),
      ));
    }
    let script = scripts.remove(0);

    let stream = futures::stream::iter(script.into_iter().map(|step| match step {
      MockStep::Delta(text) => Ok(ResponseEvent::ContentDelta(ContentDeltaEvent {
        text: text.to_string(),
        index: 0,
      })),
      MockStep::Call {
        id,
        name,
        arguments,
      } => Ok(ResponseEvent::FunctionCall(FunctionCallEvent {
        id: id.to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
          name: name.to_string(),
          arguments: arguments.to_string(),
        },
      })),
      MockStep::End => Ok(ResponseEvent::EndTurn),
    }));

    Ok(Box::pin(stream))
  }

  async fn list_models(&self) -> cokra_core::model::Result<ListModelsResponse> {
    Ok(ListModelsResponse {
      object_type: "list".to_string(),
      data: vec![ModelInfo {
        id: "mock-sse-int/model".to_string(),
        object_type: "model".to_string(),
        created: 0,
        owned_by: Some("mock".to_string()),
      }],
    })
  }

  async fn validate_auth(&self) -> cokra_core::model::Result<()> {
    Ok(())
  }

  fn client(&self) -> &Client {
    &self.client
  }

  fn config(&self) -> &ProviderConfig {
    &self.config
  }
}

#[derive(Debug)]
struct ReadFileLikeHandler;

impl ToolHandler for ReadFileLikeHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    if invocation.name != "read_file" {
      return Err(FunctionCallError::ToolNotFound(invocation.name));
    }
    Ok(ToolOutput::success("hello from tool"))
  }
}

async fn build_client(provider: MockResponsesProvider) -> Arc<ModelClient> {
  let registry = Arc::new(ProviderRegistry::new());
  registry.register(provider).await;
  registry
    .set_default("mock-sse-int")
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
    model: "mock-sse-int/model".to_string(),
    temperature: None,
    max_tokens: None,
    system_prompt: None,
    enable_tools: true,
  }
}

#[tokio::test]
async fn sse_turn_with_read_file_tool() {
  let provider = MockResponsesProvider::new(vec![
    vec![
      MockStep::Delta("I'll read the file. "),
      MockStep::Call {
        id: "read_1",
        name: "read_file",
        arguments: r#"{"file_path":"demo.txt"}"#,
      },
      MockStep::End,
    ],
    vec![
      MockStep::Delta("File content: hello from tool"),
      MockStep::End,
    ],
  ]);
  let calls = provider.calls.clone();

  let model_client = build_client(provider).await;

  let mut registry = ToolRegistry::new();
  registry.register_handler("read_file", Arc::new(ReadFileLikeHandler));
  let tool_registry = Arc::new(registry);

  let session = Arc::new(Session::new());
  let (tx_event, mut rx_event) = mpsc::channel(128);

  let executor = TurnExecutor::new(
    model_client,
    tool_registry,
    session,
    tx_event,
    test_config(),
  );

  let result = executor
    .run_turn(UserInput {
      content: "read demo".to_string(),
      attachments: Vec::new(),
    })
    .await
    .expect("run turn");

  assert!(result.success);
  assert!(result.content.contains("I'll read the file."));
  assert!(result.content.contains("File content: hello from tool"));
  assert_eq!(*calls.lock().await, 2);

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
      "item_completed",
      "item_started",
      "delta",
      "item_completed",
      "turn_complete",
    ]
  );
}
