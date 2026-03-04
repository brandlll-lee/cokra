use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::FuturesOrdered;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use cokra_protocol::AgentMessageContentDeltaEvent;
use cokra_protocol::EventMsg;
use cokra_protocol::FunctionCallEvent;
use cokra_protocol::ItemCompletedEvent;
use cokra_protocol::ItemStartedEvent;
use cokra_protocol::ResponseEvent;

use crate::model::ChatRequest;
use crate::model::Message as ModelMessage;
use crate::model::ModelClient;
use crate::model::ModelError;
use crate::model::ToolCall as ModelToolCall;
use crate::model::ToolCallFunction;
use crate::model::Usage;
use crate::session::Session;
use crate::tools::context::ToolOutput;
use crate::tools::parallel::ToolCallRuntime;
use crate::tools::registry::ToolRegistry;
use crate::tools::router::ToolCall;
use crate::tools::router::ToolRouter;
use crate::tools::router::ToolRunContext;

use super::executor::TurnConfig;
use super::executor::TurnError;
use super::executor::TurnResult;
use super::response_items::ResponseItem;

#[derive(Debug)]
struct SamplingRequestResult {
  assistant_delta: String,
  function_calls: Vec<FunctionCallEvent>,
}

#[derive(Clone)]
pub struct SseTurnExecutor {
  model_client: Arc<ModelClient>,
  tool_registry: Arc<ToolRegistry>,
  tool_runtime: ToolCallRuntime,
  session: Arc<Session>,
  tx_event: mpsc::Sender<EventMsg>,
  config: TurnConfig,
  cancellation_token: CancellationToken,
}

impl SseTurnExecutor {
  pub fn new(
    model_client: Arc<ModelClient>,
    tool_registry: Arc<ToolRegistry>,
    tool_router: Arc<ToolRouter>,
    session: Arc<Session>,
    tx_event: mpsc::Sender<EventMsg>,
    config: TurnConfig,
  ) -> Self {
    Self::new_with_cancellation(
      model_client,
      tool_registry,
      tool_router,
      session,
      tx_event,
      config,
      CancellationToken::new(),
    )
  }

  pub fn new_with_cancellation(
    model_client: Arc<ModelClient>,
    tool_registry: Arc<ToolRegistry>,
    tool_router: Arc<ToolRouter>,
    session: Arc<Session>,
    tx_event: mpsc::Sender<EventMsg>,
    config: TurnConfig,
    cancellation_token: CancellationToken,
  ) -> Self {
    Self {
      model_client,
      tool_registry,
      tool_runtime: ToolCallRuntime::new(tool_router),
      session,
      tx_event,
      config,
      cancellation_token,
    }
  }

  async fn try_run_sampling_request(
    &self,
    messages: Vec<ModelMessage>,
    thread_id: &str,
    turn_id: &str,
    item_id: &str,
    cancellation_token: CancellationToken,
  ) -> Result<SamplingRequestResult, TurnError> {
    let request = ChatRequest {
      model: self.config.model.clone(),
      messages,
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

    let mut stream = self
      .model_client
      .responses_stream(request)
      .await
      .map_err(map_stream_model_error)?;
    let mut assistant_delta = String::new();
    let mut function_calls: Vec<FunctionCallEvent> = Vec::new();

    loop {
      let event = tokio::select! {
        _ = cancellation_token.cancelled() => return Err(TurnError::TurnAborted),
        event = stream.next() => event,
      };
      let Some(event) = event else {
        break;
      };

      let event = event.map_err(map_stream_model_error)?;
      match event {
        ResponseEvent::ContentDelta(delta) => {
          if delta.text.is_empty() {
            continue;
          }
          assistant_delta.push_str(&delta.text);
          self
            .send_event(EventMsg::AgentMessageContentDelta(
              AgentMessageContentDeltaEvent {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item_id: item_id.to_string(),
                delta: delta.text,
              },
            ))
            .await?;
        }
        ResponseEvent::FunctionCall(call) => {
          function_calls.push(call);
        }
        ResponseEvent::Completed {
          token_usage: Some(usage),
          ..
        } => {
          self
            .session
            .set_token_usage(&Usage {
              input_tokens: usage.input_tokens.max(0) as u32,
              output_tokens: usage.output_tokens.max(0) as u32,
              total_tokens: usage.total_tokens.max(0) as u32,
            })
            .await;
        }
        ResponseEvent::RateLimits(_) => {
          // phase-6: reserved for future UI/state integration
        }
        ResponseEvent::EndTurn => break,
        ResponseEvent::Error(err) => {
          return Err(TurnError::ModelError(ModelError::StreamError(err.message)));
        }
        _ => {}
      }
    }

    Ok(SamplingRequestResult {
      assistant_delta,
      function_calls,
    })
  }

  async fn run_sampling_request(
    &self,
    messages: Vec<ModelMessage>,
    thread_id: &str,
    turn_id: &str,
    item_id: &str,
    cancellation_token: CancellationToken,
  ) -> Result<SamplingRequestResult, TurnError> {
    let max_retries = 3;
    let mut retries = 0;

    loop {
      match self
        .try_run_sampling_request(
          messages.clone(),
          thread_id,
          turn_id,
          item_id,
          cancellation_token.child_token(),
        )
        .await
      {
        Ok(output) => return Ok(output),
        Err(err) if err.is_retryable() && retries < max_retries => {
          retries += 1;
          let delay = backoff(retries);
          self
            .send_event(EventMsg::Warning(cokra_protocol::WarningEvent {
              thread_id: thread_id.to_string(),
              turn_id: turn_id.to_string(),
              message: format!("Reconnecting... {retries}/{max_retries}"),
            }))
            .await?;
          tokio::time::sleep(delay).await;
        }
        Err(err) => return Err(err),
      }
    }
  }

  async fn maybe_run_auto_compact_for_messages(&self, messages: &mut Vec<ModelMessage>) {
    let Some(limit) = self.config.auto_compact_token_limit else {
      return;
    };

    if estimate_messages_tokens(messages) < limit {
      return;
    }

    self.session.compact_history_to_token_target(limit).await;

    let mut rebuilt = Vec::new();
    if let Some(system) = &self.config.system_prompt {
      rebuilt.push(ModelMessage::System(system.clone()));
    }

    if let Some(context_limit) = self.config.context_window_limit {
      rebuilt.extend(self.session.get_history_for_prompt(context_limit).await);
    } else {
      rebuilt.extend(self.session.get_history(100).await);
    }

    *messages = rebuilt;
  }

  pub async fn run_sse_interaction(
    &self,
    mut messages: Vec<ModelMessage>,
    thread_id: String,
    turn_id: String,
  ) -> Result<TurnResult, TurnError> {
    let mut final_content = String::new();
    let turn_cancellation = self.cancellation_token.child_token();

    loop {
      if turn_cancellation.is_cancelled() {
        return Err(TurnError::TurnAborted);
      }

      self
        .maybe_run_auto_compact_for_messages(&mut messages)
        .await;

      let item_id = Uuid::new_v4().to_string();
      self
        .send_event(EventMsg::ItemStarted(ItemStartedEvent {
          thread_id: thread_id.clone(),
          turn_id: turn_id.clone(),
          item_id: item_id.clone(),
          item_type: "agent-message".to_string(),
        }))
        .await?;

      let SamplingRequestResult {
        assistant_delta,
        function_calls,
      } = self
        .run_sampling_request(
          messages.clone(),
          &thread_id,
          &turn_id,
          &item_id,
          turn_cancellation.child_token(),
        )
        .await?;

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
      self.session.append_message(assistant_message.clone()).await;
      if let Some(item) = ResponseItem::from_model_message(&assistant_message) {
        self.session.append_response_item(item).await;
      }

      if !function_calls.is_empty() {
        let call_items = function_calls
          .iter()
          .map(ResponseItem::from_function_call_event)
          .collect::<Vec<_>>();
        self.session.append_response_items(call_items).await;
      }

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

      let mut in_flight = FuturesOrdered::new();
      for call in function_calls {
        in_flight.push_back(self.execute_tool_call(
          call,
          &thread_id,
          &turn_id,
          turn_cancellation.child_token(),
        ));
      }

      while let Some(output_res) = in_flight.next().await {
        let (call_id, output) = output_res?;
        let output_call_id = if output.id.is_empty() {
          call_id.clone()
        } else {
          output.id
        };

        let tool_msg = ModelMessage::Tool {
          tool_call_id: output_call_id,
          content: output.content,
        };
        messages.push(tool_msg.clone());
        self.session.append_message(tool_msg.clone()).await;
        self
          .session
          .append_response_item(ResponseItem::FunctionCallOutput {
            call_id,
            output: tool_msg.text().unwrap_or_default().to_string(),
            is_error: false,
          })
          .await;
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

  async fn execute_tool_call(
    &self,
    call: FunctionCallEvent,
    thread_id: &str,
    turn_id: &str,
    cancellation_token: CancellationToken,
  ) -> Result<(String, ToolOutput), TurnError> {
    let args = serde_json::from_str::<serde_json::Value>(&call.function.arguments)
      .map_err(|err| TurnError::ToolError(format!("invalid tool arguments: {err}")))?;

    let tool_call = ToolCall {
      tool_name: call.function.name.clone(),
      call_id: call.id.clone(),
      args,
    };
    let mut run_ctx = ToolRunContext::new(
      Arc::clone(&self.session),
      thread_id.to_string(),
      turn_id.to_string(),
      self.config.cwd.clone(),
      self.config.approval_policy.clone(),
      self.config.sandbox_policy.clone(),
    );
    run_ctx.has_managed_network_requirements = self.config.has_managed_network_requirements;
    run_ctx.tx_event = Some(self.tx_event.clone());

    let mut output = self
      .tool_runtime
      .handle_tool_call_with_cancellation(tool_call, run_ctx, cancellation_token)
      .await
      .map_err(|err| TurnError::ToolError(err.to_string()))?;

    if output.id.is_empty() {
      output.id = call.id.clone();
    }

    Ok((call.id, output))
  }

  async fn send_event(&self, event: EventMsg) -> Result<(), TurnError> {
    self.session.emit_event(event.clone());
    self
      .tx_event
      .send(event)
      .await
      .map_err(|err| TurnError::SessionError(format!("failed to send event: {err}")))
  }

  pub fn cancel_current_turn(&self) {
    self.cancellation_token.cancel();
  }
}

fn estimate_message_tokens(msg: &ModelMessage) -> usize {
  let text_len = match msg {
    ModelMessage::System(text) | ModelMessage::User(text) => text.chars().count(),
    ModelMessage::Assistant { content, .. } => content.as_deref().map_or(0, |s| s.chars().count()),
    ModelMessage::Tool { content, .. } => content.chars().count(),
  };
  if text_len == 0 {
    1
  } else {
    text_len.div_ceil(4)
  }
}

fn estimate_messages_tokens(messages: &[ModelMessage]) -> usize {
  messages.iter().map(estimate_message_tokens).sum()
}

fn map_stream_model_error(err: ModelError) -> TurnError {
  match err {
    ModelError::StreamError(msg) => TurnError::Stream(msg, None),
    other => TurnError::ModelError(other),
  }
}

fn backoff(retries: usize) -> Duration {
  let seconds = 2u64.pow((retries.min(5) as u32).saturating_sub(1));
  Duration::from_secs(seconds.max(1))
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

  use cokra_config::ApprovalMode;
  use cokra_config::ApprovalPolicy;
  use cokra_config::PatchApproval;
  use cokra_config::SandboxConfig;
  use cokra_config::SandboxMode;
  use cokra_config::ShellApproval;
  use cokra_protocol::ContentDeltaEvent;
  use cokra_protocol::FunctionCall;
  use cokra_protocol::ResponseErrorEvent;

  use super::SseTurnExecutor;
  use crate::model::ChatRequest;
  use crate::model::ChatResponse;
  use crate::model::Chunk;
  use crate::model::ListModelsResponse;
  use crate::model::Message as ModelMessage;
  use crate::model::ModelClient;
  use crate::model::ModelError;
  use crate::model::ModelInfo;
  use crate::model::ProviderConfig;
  use crate::model::ProviderRegistry;
  use crate::model::provider::ModelProvider;
  use crate::session::Session;
  use crate::tools::context::FunctionCallError;
  use crate::tools::context::ToolInvocation;
  use crate::tools::context::ToolOutput;
  use crate::tools::registry::ToolHandler;
  use crate::tools::registry::ToolKind;
  use crate::tools::registry::ToolRegistry;
  use crate::tools::router::ToolRouter;
  use crate::tools::validation::ToolValidator;
  use crate::turn::TurnConfig;
  use crate::turn::TurnError;
  use cokra_protocol::EventMsg;
  use cokra_protocol::FunctionCallEvent;
  use cokra_protocol::ResponseEvent;

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

  #[async_trait]
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
    let tool_router = build_router(tool_registry.clone());
    let session = Arc::new(Session::new());
    let (tx_event, rx_event) = mpsc::channel(64);

    let executor = SseTurnExecutor::new(
      model_client,
      tool_registry,
      tool_router,
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
    let tool_router = build_router(tool_registry.clone());

    let session = Arc::new(Session::new());
    let (tx_event, rx_event) = mpsc::channel(64);

    let executor = SseTurnExecutor::new(
      model_client,
      tool_registry,
      tool_router,
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
    let tool_router = build_router(tool_registry.clone());
    let session = Arc::new(Session::new());
    let (tx_event, rx_event) = mpsc::channel(64);

    let executor = SseTurnExecutor::new(
      model_client,
      tool_registry,
      tool_router,
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
    let tool_router = build_router(tool_registry.clone());
    let session = Arc::new(Session::new());
    let (tx_event, _rx_event) = mpsc::channel(64);

    let executor = SseTurnExecutor::new(
      model_client,
      tool_registry,
      tool_router,
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
