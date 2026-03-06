use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct SpawnAgentHandler;

#[derive(Debug, Deserialize)]
struct SpawnAgentArgs {
  task: Option<String>,
  message: Option<String>,
  role: Option<String>,
  agent_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct SpawnAgentResult {
  thread_id: String,
  agent_id: String,
  role: String,
  status: String,
}

fn resolve_message(args: SpawnAgentArgs) -> Result<(String, String), FunctionCallError> {
  let task = args.task.map(|value| value.trim().to_string());
  let message = args.message.map(|value| value.trim().to_string());

  let message = match (task, message) {
    (Some(task), Some(message)) if !task.is_empty() && !message.is_empty() => {
      return Err(FunctionCallError::RespondToModel(
        "spawn_agent accepts either `task` or `message`, not both".to_string(),
      ));
    }
    (Some(task), _) if !task.is_empty() => task,
    (_, Some(message)) if !message.is_empty() => message,
    _ => {
      return Err(FunctionCallError::RespondToModel(
        "spawn_agent requires a non-empty `task` or `message`".to_string(),
      ));
    }
  };

  let role = args
    .agent_type
    .or(args.role)
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| "default".to_string());

  Ok((message, role))
}

#[async_trait]
impl ToolHandler for SpawnAgentHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: SpawnAgentArgs = invocation.parse_arguments()?;
    let runtime = invocation
      .runtime
      .ok_or_else(|| FunctionCallError::Fatal("spawn_agent missing runtime context".to_string()))?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("spawn_agent runtime is not configured".to_string())
    })?;
    let (message, role) = resolve_message(args)?;
    let thread_id = team_runtime
      .spawn_agent(&runtime.thread_id, message, role.clone())
      .await
      .map_err(|err| FunctionCallError::Execution(err.to_string()))?;

    let mut out = ToolOutput::success(
      serde_json::to_string(&SpawnAgentResult {
        thread_id: thread_id.to_string(),
        agent_id: thread_id.to_string(),
        role,
        status: "running".to_string(),
      })
      .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize spawn result: {err}"))
      })?,
    );
    out.id = invocation.id;
    Ok(out)
  }
}
