use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::mpsc;
use uuid::Uuid;

use cokra_protocol::{
  AgentMessageContentDeltaEvent, EventMsg, FunctionCallEvent, ItemCompletedEvent, ItemStartedEvent,
  ResponseEvent,
};

use crate::model::{
  ChatRequest, Message as ModelMessage, ModelClient, ModelError, ToolCall as ModelToolCall,
  ToolCallFunction, Usage,
};
use crate::session::Session;
use crate::tools::context::{ToolInvocation, ToolOutput};
use crate::tools::registry::ToolRegistry;

use super::executor::{TurnConfig, TurnError, TurnResult};

#[derive(Clone)]
pub struct SseTurnExecutor {
  model_client: Arc<ModelClient>,
  tool_registry: Arc<ToolRegistry>,
  session: Arc<Session>,
  tx_event: mpsc::Sender<EventMsg>,
  config: TurnConfig,
}

impl SseTurnExecutor {
  pub fn new(
    model_client: Arc<ModelClient>,
    tool_registry: Arc<ToolRegistry>,
    session: Arc<Session>,
    tx_event: mpsc::Sender<EventMsg>,
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

  pub async fn run_sse_interaction(
    &self,
    mut messages: Vec<ModelMessage>,
    thread_id: String,
    turn_id: String,
  ) -> Result<TurnResult, TurnError> {
    let mut final_content = String::new();
    let max_iterations = 10;

    for _ in 0..max_iterations {
      let item_id = Uuid::new_v4().to_string();
      self
        .send_event(EventMsg::ItemStarted(ItemStartedEvent {
          thread_id: thread_id.clone(),
          turn_id: turn_id.clone(),
          item_id: item_id.clone(),
          item_type: "agent-message".to_string(),
        }))
        .await?;

      let request = ChatRequest {
        model: self.config.model.clone(),
        messages: messages.clone(),
        temperature: self.config.temperature,
        max_tokens: self.config.max_tokens,
        tools: if self.config.enable_tools {
          Some(self.tool_registry.model_tools())
        } else {
          None
        },
        stream: true,
        ..Default::default()
      };

      let mut stream = self.model_client.responses_stream(request).await?;

      let mut assistant_delta = String::new();
      let mut function_calls: Vec<FunctionCallEvent> = Vec::new();

      while let Some(event) = stream.next().await {
        match event? {
          ResponseEvent::ContentDelta(delta) => {
            if delta.text.is_empty() {
              continue;
            }
            assistant_delta.push_str(&delta.text);
            self
              .send_event(EventMsg::AgentMessageContentDelta(
                AgentMessageContentDeltaEvent {
                  thread_id: thread_id.clone(),
                  turn_id: turn_id.clone(),
                  item_id: item_id.clone(),
                  delta: delta.text,
                },
              ))
              .await?;
          }
          ResponseEvent::FunctionCall(call) => {
            function_calls.push(call);
          }
          ResponseEvent::EndTurn => break,
          ResponseEvent::Error(err) => {
            return Err(TurnError::ModelError(ModelError::StreamError(err.message)));
          }
        }
      }

      if !assistant_delta.is_empty() {
        final_content.push_str(&assistant_delta);
      }

      let assistant_message = ModelMessage::Assistant {
        content: if assistant_delta.is_empty() {
          None
        } else {
          Some(assistant_delta.clone())
        },
        tool_calls: if function_calls.is_empty() {
          None
        } else {
          Some(
            function_calls
              .iter()
              .map(Self::to_model_tool_call)
              .collect::<Vec<_>>(),
          )
        },
      };
      messages.push(assistant_message.clone());
      self.session.append_message(assistant_message).await;

      if function_calls.is_empty() {
        self
          .send_event(EventMsg::ItemCompleted(ItemCompletedEvent {
            thread_id: thread_id.clone(),
            turn_id: turn_id.clone(),
            item_id,
            result: final_content.clone(),
          }))
          .await?;

        return Ok(TurnResult {
          content: final_content,
          usage: Usage::default(),
          success: true,
        });
      }

      for call in function_calls {
        let output = self.execute_tool_call(&call).await?;
        let output_call_id = if output.id.is_empty() {
          call.id
        } else {
          output.id
        };

        let tool_msg = ModelMessage::Tool {
          tool_call_id: output_call_id,
          content: output.content,
        };
        messages.push(tool_msg.clone());
        self.session.append_message(tool_msg).await;
      }

      self
        .send_event(EventMsg::ItemCompleted(ItemCompletedEvent {
          thread_id: thread_id.clone(),
          turn_id: turn_id.clone(),
          item_id,
          result: assistant_delta,
        }))
        .await?;
    }

    Err(TurnError::SessionError(
      "too many tool call iterations".to_string(),
    ))
  }

  fn to_model_tool_call(call: &FunctionCallEvent) -> ModelToolCall {
    ModelToolCall {
      id: call.id.clone(),
      call_type: call.call_type.clone(),
      function: ToolCallFunction {
        name: call.function.name.clone(),
        arguments: call.function.arguments.clone(),
      },
    }
  }

  async fn execute_tool_call(&self, call: &FunctionCallEvent) -> Result<ToolOutput, TurnError> {
    let handler = self
      .tool_registry
      .get_handler(&call.function.name)
      .ok_or_else(|| TurnError::ToolNotFound(call.function.name.clone()))?;

    let invocation = ToolInvocation {
      id: call.id.clone(),
      name: call.function.name.clone(),
      arguments: call.function.arguments.clone(),
    };

    let mut output = handler
      .handle(invocation)
      .map_err(|err| TurnError::ToolError(err.to_string()))?;

    if output.id.is_empty() {
      output.id = call.id.clone();
    }

    Ok(output)
  }

  async fn send_event(&self, event: EventMsg) -> Result<(), TurnError> {
    self.session.emit_event(event.clone());
    self
      .tx_event
      .send(event)
      .await
      .map_err(|err| TurnError::SessionError(format!("failed to send event: {err}")))
  }
}

#[cfg(test)]
mod tests {
  use std::pin::Pin;
  use std::sync::Arc;

  use async_trait::async_trait;
  use futures::Stream;
  use reqwest::Client;
  use tokio::sync::{Mutex, mpsc};

  use cokra_protocol::{ContentDeltaEvent, FunctionCall, ResponseErrorEvent};

  use super::SseTurnExecutor;
  use crate::model::provider::ModelProvider;
  use crate::model::{
    ChatRequest, ChatResponse, Chunk, ListModelsResponse, Message as ModelMessage, ModelClient,
    ModelError, ModelInfo, ProviderConfig, ProviderRegistry,
  };
  use crate::session::Session;
  use crate::tools::context::{FunctionCallError, ToolInvocation, ToolOutput};
  use crate::tools::registry::{ToolHandler, ToolKind, ToolRegistry};
  use crate::turn::{TurnConfig, TurnError};
  use cokra_protocol::{EventMsg, FunctionCallEvent, ResponseEvent};

  #[derive(Debug)]
  enum MockStep {
    Delta(&'static str),
    Call {
      id: &'static str,
      name: &'static str,
      arguments: &'static str,
    },
    Error(&'static str),
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
          provider_id: "mock-sse".to_string(),
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
      "mock-sse"
    }

    fn provider_name(&self) -> &'static str {
      "Mock Responses Provider"
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
      request: ChatRequest,
    ) -> crate::model::Result<Pin<Box<dyn Stream<Item = crate::model::Result<ResponseEvent>> + Send>>>
    {
      let mut calls = self.calls.lock().await;
      *calls += 1;

      if *calls == 2 {
        let saw_tool_output = request.messages.iter().any(|msg| {
          matches!(msg, ModelMessage::Tool { tool_call_id, content } if tool_call_id == "read_1" && content.contains("hello from tool"))
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
        MockStep::Error(message) => Ok(ResponseEvent::Error(ResponseErrorEvent {
          message: message.to_string(),
        })),
        MockStep::End => Ok(ResponseEvent::EndTurn),
      }));

      Ok(Box::pin(stream))
    }

    async fn list_models(&self) -> crate::model::Result<ListModelsResponse> {
      Ok(ListModelsResponse {
        object_type: "list".to_string(),
        data: vec![ModelInfo {
          id: "mock-sse/model".to_string(),
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
      .set_default("mock-sse")
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
      model: "mock-sse/model".to_string(),
      temperature: None,
      max_tokens: None,
      system_prompt: None,
      enable_tools: true,
    }
  }

  fn collect_events(mut rx_event: mpsc::Receiver<EventMsg>) -> Vec<EventMsg> {
    let mut events = Vec::new();
    while let Ok(event) = rx_event.try_recv() {
      events.push(event);
    }
    events
  }

  #[tokio::test]
  async fn test_sse_content_delta() {
    let provider = MockResponsesProvider::new(vec![vec![
      MockStep::Delta("Hello"),
      MockStep::Delta(" World"),
      MockStep::End,
    ]]);

    let model_client = build_client(provider).await;
    let tool_registry = Arc::new(ToolRegistry::new());
    let session = Arc::new(Session::new());
    let (tx_event, rx_event) = mpsc::channel(64);

    let executor = SseTurnExecutor::new(
      model_client,
      tool_registry,
      session,
      tx_event,
      test_config(),
    );

    let result = executor
      .run_sse_interaction(
        vec![ModelMessage::User("hello".to_string())],
        "thread-1".to_string(),
        "turn-1".to_string(),
      )
      .await
      .expect("sse run");

    assert_eq!(result.content, "Hello World");
    assert!(result.success);

    let events = collect_events(rx_event);
    let delta_count = events
      .iter()
      .filter(|event| matches!(event, EventMsg::AgentMessageContentDelta(_)))
      .count();
    assert_eq!(delta_count, 2);
  }

  #[tokio::test]
  async fn test_sse_tool_call_loop() {
    let provider = MockResponsesProvider::new(vec![
      vec![
        MockStep::Delta("I'll read it. "),
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
    let (tx_event, rx_event) = mpsc::channel(64);

    let executor = SseTurnExecutor::new(
      model_client,
      tool_registry,
      session,
      tx_event,
      test_config(),
    );

    let result = executor
      .run_sse_interaction(
        vec![ModelMessage::User("read demo".to_string())],
        "thread-2".to_string(),
        "turn-2".to_string(),
      )
      .await
      .expect("sse run");

    assert!(result.content.contains("I'll read it."));
    assert!(result.content.contains("File content: hello from tool"));
    assert_eq!(*calls.lock().await, 2);

    let events = collect_events(rx_event);
    let item_started = events
      .iter()
      .filter(|event| matches!(event, EventMsg::ItemStarted(_)))
      .count();
    let item_completed = events
      .iter()
      .filter(|event| matches!(event, EventMsg::ItemCompleted(_)))
      .count();

    assert_eq!(item_started, 2);
    assert_eq!(item_completed, 2);
  }

  #[tokio::test]
  async fn test_sse_event_ordering() {
    let provider = MockResponsesProvider::new(vec![vec![
      MockStep::Delta("Hello"),
      MockStep::Delta(" world"),
      MockStep::End,
    ]]);

    let model_client = build_client(provider).await;
    let tool_registry = Arc::new(ToolRegistry::new());
    let session = Arc::new(Session::new());
    let (tx_event, rx_event) = mpsc::channel(64);

    let executor = SseTurnExecutor::new(
      model_client,
      tool_registry,
      session,
      tx_event,
      test_config(),
    );

    executor
      .run_sse_interaction(
        vec![ModelMessage::User("hello".to_string())],
        "thread-3".to_string(),
        "turn-3".to_string(),
      )
      .await
      .expect("sse run");

    let events = collect_events(rx_event);
    let order = events
      .iter()
      .map(|event| match event {
        EventMsg::ItemStarted(_) => "item_started",
        EventMsg::AgentMessageContentDelta(_) => "delta",
        EventMsg::ItemCompleted(_) => "item_completed",
        _ => "other",
      })
      .collect::<Vec<_>>();

    assert_eq!(
      order,
      vec!["item_started", "delta", "delta", "item_completed"]
    );
  }

  #[tokio::test]
  async fn test_sse_error_event_returns_turn_error() {
    let provider = MockResponsesProvider::new(vec![vec![MockStep::Error("boom")]]);
    let model_client = build_client(provider).await;
    let tool_registry = Arc::new(ToolRegistry::new());
    let session = Arc::new(Session::new());
    let (tx_event, _rx_event) = mpsc::channel(64);

    let executor = SseTurnExecutor::new(
      model_client,
      tool_registry,
      session,
      tx_event,
      test_config(),
    );

    let result = executor
      .run_sse_interaction(
        vec![ModelMessage::User("hi".to_string())],
        "thread-4".to_string(),
        "turn-4".to_string(),
      )
      .await;

    match result {
      Err(TurnError::ModelError(ModelError::StreamError(message))) => {
        assert_eq!(message, "boom");
      }
      _ => panic!("expected stream error from SSE response"),
    }
  }
}
