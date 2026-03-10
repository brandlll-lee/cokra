use async_trait::async_trait;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use cokra_protocol::RequestUserInputArgs;

pub struct RequestUserInputHandler;

#[async_trait]
impl ToolHandler for RequestUserInputHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let mut args: RequestUserInputArgs = invocation.parse_arguments()?;
    let runtime = invocation.runtime.ok_or_else(|| {
      FunctionCallError::Fatal("request_user_input missing runtime context".to_string())
    })?;
    if let Some(team_runtime) = runtime_for_thread(&runtime.thread_id)
      && !team_runtime.is_root_thread(&runtime.thread_id)
    {
      return Err(FunctionCallError::RespondToModel(
        "request_user_input is unavailable for spawned teammate agents; make a reasonable assumption or report the missing information back to @main".to_string(),
      ));
    }
    let missing_options = args
      .questions
      .iter()
      .any(|question| question.options.as_ref().is_none_or(Vec::is_empty));
    if missing_options {
      return Err(FunctionCallError::RespondToModel(
        "request_user_input requires non-empty options for every question".to_string(),
      ));
    }
    for question in &mut args.questions {
      question.is_other = true;
    }
    let response = runtime
      .session
      .request_user_input(
        runtime.thread_id.clone(),
        runtime.turn_id.clone(),
        runtime.turn_id.clone(),
        invocation.id.clone(),
        args.questions,
        runtime.tx_event.clone(),
      )
      .await
      .ok_or_else(|| {
        FunctionCallError::RespondToModel(
          "request_user_input was cancelled before receiving a response".to_string(),
        )
      })?;
    let content = serde_json::to_string(&response).map_err(|err| {
      FunctionCallError::Fatal(format!(
        "failed to serialize request_user_input response: {err}"
      ))
    })?;
    let mut out = ToolOutput::success(content);
    out.id = invocation.id;
    Ok(out)
  }
}
