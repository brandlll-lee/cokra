use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

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
use cokra_core::model::ChatRequest;
use cokra_core::model::ChatResponse;
use cokra_core::model::Chunk;
use cokra_core::model::ListModelsResponse;
use cokra_core::model::Message;
use cokra_core::model::ModelClient;
use cokra_core::model::ModelError;
use cokra_core::model::ModelInfo;
use cokra_core::model::ModelProvider;
use cokra_core::model::ProviderConfig;
use cokra_core::model::ProviderRegistry;
use cokra_core::session::Session;
use cokra_core::tools::context::FunctionCallError;
use cokra_core::tools::context::ToolInvocation;
use cokra_core::tools::context::ToolOutput;
use cokra_core::tools::registry::ToolHandler;
use cokra_core::tools::registry::ToolKind;
use cokra_core::tools::registry::ToolRegistry;
use cokra_core::tools::router::ToolRouter;
use cokra_core::tools::validation::ToolValidator;
use cokra_core::turn::TurnConfig;
use cokra_core::turn::TurnExecutor;
use cokra_core::turn::UserInput;

use cokra_protocol::AskForApproval;
use cokra_protocol::ContentDeltaEvent;
use cokra_protocol::EventMsg;
use cokra_protocol::FunctionCall;
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
  End,
}

#[derive(Debug)]
struct MockResponsesProvider {
  client: Client,
  config: ProviderConfig,
  scripts: Arc<Mutex<Vec<Vec<MockStep>>>>,
  calls: Arc<Mutex<u32>>,
  expected_tool_output: Option<(String, String)>,
}

impl MockResponsesProvider {
  fn new(scripts: Vec<Vec<MockStep>>) -> Self {
    Self::new_with_expectation(scripts, Some(("read_1", "hello from tool")))
  }

  fn new_with_expectation(
    scripts: Vec<Vec<MockStep>>,
    expected_tool_output: Option<(&str, &str)>,
  ) -> Self {
    Self {
      client: Client::new(),
      config: ProviderConfig {
        provider_id: "mock-sse-int".to_string(),
        ..Default::default()
      },
      scripts: Arc::new(Mutex::new(scripts)),
      calls: Arc::new(Mutex::new(0)),
      expected_tool_output: expected_tool_output
        .map(|(id, content)| (id.to_string(), content.to_string())),
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
      let expected = self.expected_tool_output.clone();
      let saw_tool_output = request.messages.iter().any(|msg| {
        if let Some((expected_id, expected_content)) = expected.as_ref() {
          matches!(msg, Message::Tool { tool_call_id, content } if tool_call_id == expected_id && content.contains(expected_content))
        } else {
          false
        }
      });
      if expected.is_some() && !saw_tool_output {
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

#[derive(Debug)]
struct FlakyWriteHandler {
  calls: Arc<AtomicUsize>,
}

impl ToolHandler for FlakyWriteHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    true
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    if invocation.name != "write_file" {
      return Err(FunctionCallError::ToolNotFound(invocation.name));
    }
    let n = self.calls.fetch_add(1, Ordering::SeqCst);
    if n == 0 {
      return Err(FunctionCallError::Execution("sandbox denied".to_string()));
    }
    Ok(ToolOutput::success("write ok"))
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
  let tool_router = build_router(tool_registry.clone());

  let session = Arc::new(Session::new());
  let (tx_event, mut rx_event) = mpsc::channel(128);

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

#[tokio::test]
async fn sse_turn_retries_after_sandbox_denied_when_policy_allows() {
  let provider = MockResponsesProvider::new_with_expectation(
    vec![
      vec![
        MockStep::Delta("Trying write. "),
        MockStep::Call {
          id: "write_1",
          name: "write_file",
          arguments: r#"{"file_path":"demo.txt","content":"hello"}"#,
        },
        MockStep::End,
      ],
      vec![MockStep::Delta("write complete"), MockStep::End],
    ],
    Some(("write_1", "write ok")),
  );

  let model_client = build_client(provider).await;
  let mut registry = ToolRegistry::new();
  let write_calls = Arc::new(AtomicUsize::new(0));
  registry.register_handler(
    "write_file",
    Arc::new(FlakyWriteHandler {
      calls: Arc::clone(&write_calls),
    }),
  );
  let tool_registry = Arc::new(registry);
  let tool_router = build_router(tool_registry.clone());

  let session = Arc::new(Session::new());
  let (tx_event, mut rx_event) = mpsc::channel(128);

  let mut config = test_config();
  config.approval_policy = AskForApproval::UnlessTrusted;

  let executor = TurnExecutor::new(
    model_client,
    tool_registry,
    tool_router,
    session,
    tx_event,
    config,
  );
  let result = executor
    .run_turn(UserInput {
      content: "write demo".to_string(),
      attachments: Vec::new(),
    })
    .await
    .expect("run turn");

  assert!(result.success);
  assert!(result.content.contains("write complete"));
  assert_eq!(write_calls.load(Ordering::SeqCst), 2);

  let mut saw_approval = false;
  while let Ok(event) = rx_event.try_recv() {
    if matches!(event, EventMsg::ExecApprovalRequest(_)) {
      saw_approval = true;
    }
  }
  assert!(saw_approval, "expected approval event for write_file");
}

#[tokio::test]
async fn sse_turn_supports_deferred_network_approval_path() {
  let provider = MockResponsesProvider::new_with_expectation(
    vec![
      vec![
        MockStep::Delta("Checking network path. "),
        MockStep::Call {
          id: "read_2",
          name: "read_file",
          arguments: r#"{"file_path":"demo.txt","__network_approval_mode":"deferred"}"#,
        },
        MockStep::End,
      ],
      vec![MockStep::Delta("deferred done"), MockStep::End],
    ],
    Some(("read_2", "hello from tool")),
  );

  let model_client = build_client(provider).await;
  let mut registry = ToolRegistry::new();
  registry.register_handler("read_file", Arc::new(ReadFileLikeHandler));
  let tool_registry = Arc::new(registry);
  let tool_router = build_router(tool_registry.clone());

  let session = Arc::new(Session::new());
  let (tx_event, mut rx_event) = mpsc::channel(128);

  let mut config = test_config();
  config.has_managed_network_requirements = true;

  let executor = TurnExecutor::new(
    model_client,
    tool_registry,
    tool_router,
    session,
    tx_event,
    config,
  );
  let result = executor
    .run_turn(UserInput {
      content: "read demo deferred".to_string(),
      attachments: Vec::new(),
    })
    .await
    .expect("run turn");

  assert!(result.success);
  assert!(result.content.contains("deferred done"));

  let mut begin_count = 0usize;
  let mut end_count = 0usize;
  while let Ok(event) = rx_event.try_recv() {
    match event {
      EventMsg::ExecCommandBegin(_) => begin_count += 1,
      EventMsg::ExecCommandEnd(_) => end_count += 1,
      _ => {}
    }
  }
  assert!(begin_count >= 1);
  assert!(end_count >= 1);
}
