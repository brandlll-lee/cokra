use std::sync::{Arc, Mutex, OnceLock};

use serde::Deserialize;

use cokra_protocol::ThreadId;

use crate::agent::AgentControl;
use crate::tools::context::{FunctionCallError, ToolInvocation, ToolOutput};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct SpawnAgentHandler;

#[derive(Debug, Deserialize)]
struct SpawnAgentArgs {
  task: String,
  role: Option<String>,
}

#[derive(Clone)]
struct SpawnAgentRuntime {
  agent_control: Arc<AgentControl>,
  parent_thread_id: ThreadId,
  max_threads: Option<usize>,
  depth: usize,
}

static SPAWN_RUNTIME: OnceLock<Mutex<Option<SpawnAgentRuntime>>> = OnceLock::new();

fn spawn_runtime() -> &'static Mutex<Option<SpawnAgentRuntime>> {
  SPAWN_RUNTIME.get_or_init(|| Mutex::new(None))
}

pub fn configure_spawn_agent_runtime(
  agent_control: Arc<AgentControl>,
  parent_thread_id: ThreadId,
  max_threads: Option<usize>,
  depth: usize,
) {
  let mut slot = spawn_runtime()
    .lock()
    .unwrap_or_else(std::sync::PoisonError::into_inner);
  *slot = Some(SpawnAgentRuntime {
    agent_control,
    parent_thread_id,
    max_threads,
    depth,
  });
}

pub fn clear_spawn_agent_runtime() {
  let mut slot = spawn_runtime()
    .lock()
    .unwrap_or_else(std::sync::PoisonError::into_inner);
  *slot = None;
}

impl ToolHandler for SpawnAgentHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
    let args: SpawnAgentArgs = invocation.parse_arguments()?;
    let runtime = spawn_runtime()
      .lock()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .clone()
      .ok_or_else(|| {
        FunctionCallError::Execution("spawn_agent runtime is not configured".to_string())
      })?;

    let role = args.role.unwrap_or_else(|| "default".to_string());
    let task = args.task;
    let agent_control = runtime.agent_control;
    let parent_thread_id = runtime.parent_thread_id;
    let max_threads = runtime.max_threads;
    let depth = runtime.depth;
    let spawn_role = role.clone();

    let thread_id = std::thread::spawn(move || {
      let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| FunctionCallError::Execution(format!("failed to create runtime: {e}")))?;
      rt.block_on(agent_control.spawn_agent(
        task,
        Some(spawn_role),
        Some(parent_thread_id),
        depth,
        max_threads,
      ))
      .map_err(|e| FunctionCallError::Execution(e.to_string()))
    })
    .join()
    .map_err(|_| {
      FunctionCallError::Execution("spawn_agent worker thread panicked".to_string())
    })??;

    let mut out = ToolOutput::success(
      serde_json::json!({
        "thread_id": thread_id.to_string(),
        "agent_id": thread_id.to_string(),
        "role": role,
        "status": "created",
      })
      .to_string(),
    );
    out.id = invocation.id;
    Ok(out)
  }
}
