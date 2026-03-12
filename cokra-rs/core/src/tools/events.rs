use std::path::Path;

use cokra_protocol::EventMsg;
use cokra_protocol::ExecCommandBeginEvent;
use cokra_protocol::ExecCommandEndEvent;
use tokio::sync::mpsc;

use crate::exec::try_parse_model_structured_exec_output;
use crate::session::Session;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolOutput;

#[derive(Clone)]
pub struct ToolEventCtx<'a> {
  pub session: &'a Session,
  pub tx_event: Option<mpsc::Sender<EventMsg>>,
  pub thread_id: &'a str,
  pub turn_id: &'a str,
  pub call_id: &'a str,
  #[allow(dead_code)]
  pub tool_name: &'a str,
  pub cwd: &'a Path,
}

pub enum ToolEventStage {
  Begin,
  Success(ToolOutput),
  Failure(FunctionCallError),
}

/// Generic tool event emitter. cokra currently maps tool lifecycle to
/// ExecCommandBegin/End so stream consumers get deterministic begin/end events.
pub struct ToolEmitter {
  tool_name: String,
  /// 1:1 codex: actual command string for ExecCommandBegin.command.
  /// When set, this is used instead of tool_name in the Begin event
  /// so the TUI renders the real command (e.g. "pwd") not "shell".
  display_command: Option<String>,
}

impl ToolEmitter {
  pub fn new(tool_name: impl Into<String>) -> Self {
    Self {
      tool_name: tool_name.into(),
      display_command: None,
    }
  }

  /// 1:1 codex: construct emitter for shell tool with the actual command string.
  pub fn shell_with_command(raw_command: impl Into<String>) -> Self {
    Self {
      tool_name: "shell".to_string(),
      display_command: Some(raw_command.into()),
    }
  }

  pub fn with_display_command(
    tool_name: impl Into<String>,
    display_command: impl Into<String>,
  ) -> Self {
    Self {
      tool_name: tool_name.into(),
      display_command: Some(display_command.into()),
    }
  }

  pub fn shell(_command: Vec<String>) -> Self {
    Self::new("shell")
  }

  pub fn apply_patch() -> Self {
    Self::new("apply_patch")
  }

  pub async fn emit(&self, ctx: ToolEventCtx<'_>, stage: ToolEventStage) {
    match stage {
      ToolEventStage::Begin => {
        emit_event(
          &ctx,
          EventMsg::ExecCommandBegin(ExecCommandBeginEvent {
            thread_id: ctx.thread_id.to_string(),
            turn_id: ctx.turn_id.to_string(),
            command_id: ctx.call_id.to_string(),
            tool_name: self.tool_name.clone(),
            command: self
              .display_command
              .clone()
              .unwrap_or_else(|| self.tool_name.clone()),
            cwd: ctx.cwd.to_path_buf(),
          }),
        )
        .await;
      }
      ToolEventStage::Success(output) => {
        // Tradeoff: we decode the model-structured exec envelope here so the TUI
        // shows human output (not JSON) without coupling the UI layer to tool payloads.
        // This is the narrowest place that knows both "tool output format" and "UI event".
        let (exit_code, display_output) =
          if let Some(envelope) = try_parse_model_structured_exec_output(&output.text_content()) {
            (envelope.metadata.exit_code, envelope.output)
          } else {
            (if output.is_error() { 1 } else { 0 }, output.text_content())
          };

        emit_event(
          &ctx,
          EventMsg::ExecCommandEnd(ExecCommandEndEvent {
            thread_id: ctx.thread_id.to_string(),
            turn_id: ctx.turn_id.to_string(),
            command_id: ctx.call_id.to_string(),
            exit_code,
            output: display_output,
          }),
        )
        .await;
      }
      ToolEventStage::Failure(err) => {
        emit_event(
          &ctx,
          EventMsg::ExecCommandEnd(ExecCommandEndEvent {
            thread_id: ctx.thread_id.to_string(),
            turn_id: ctx.turn_id.to_string(),
            command_id: ctx.call_id.to_string(),
            exit_code: -1,
            output: err.to_string(),
          }),
        )
        .await;
      }
    }
  }

  pub async fn begin(&self, ctx: ToolEventCtx<'_>) {
    self.emit(ctx, ToolEventStage::Begin).await;
  }

  pub async fn finish(
    &self,
    ctx: ToolEventCtx<'_>,
    result: Result<ToolOutput, FunctionCallError>,
  ) -> Result<String, FunctionCallError> {
    match result {
      Ok(output) => {
        let content = output.text_content();
        self.emit(ctx, ToolEventStage::Success(output)).await;
        Ok(content)
      }
      Err(err) => {
        self.emit(ctx, ToolEventStage::Failure(err.clone())).await;
        Err(err)
      }
    }
  }
}

async fn emit_event(ctx: &ToolEventCtx<'_>, event: EventMsg) {
  ctx.session.emit_event(event.clone());
  if let Some(tx_event) = &ctx.tx_event {
    let _ = tx_event.send(event).await;
  }
}

#[cfg(test)]
mod tests {
  use std::path::Path;

  use cokra_protocol::EventMsg;

  use super::*;
  use crate::session::Session;

  #[tokio::test]
  async fn begin_then_success_event_order_is_stable() {
    let session = Session::new();
    let mut rx = session.subscribe_events();
    let emitter = ToolEmitter::new("read_file");
    let ctx = ToolEventCtx {
      session: &session,
      tx_event: None,
      thread_id: "thread-1",
      turn_id: "turn-1",
      call_id: "call-1",
      tool_name: "read_file",
      cwd: Path::new("."),
    };

    emitter.begin(ctx.clone()).await;
    let _ = emitter
      .finish(ctx, Ok(ToolOutput::success("ok").with_id("call-1")))
      .await;

    let first = rx.recv().await.expect("first event");
    let second = rx.recv().await.expect("second event");

    assert!(matches!(first, EventMsg::ExecCommandBegin(_)));
    assert!(matches!(second, EventMsg::ExecCommandEnd(_)));
  }

  #[tokio::test]
  async fn begin_then_failure_event_order_is_stable() {
    let session = Session::new();
    let mut rx = session.subscribe_events();
    let emitter = ToolEmitter::new("read_file");
    let ctx = ToolEventCtx {
      session: &session,
      tx_event: None,
      thread_id: "thread-1",
      turn_id: "turn-1",
      call_id: "call-1",
      tool_name: "read_file",
      cwd: Path::new("."),
    };

    emitter.begin(ctx.clone()).await;
    let _ = emitter
      .finish(ctx, Err(FunctionCallError::Execution("boom".to_string())))
      .await;

    let first = rx.recv().await.expect("first event");
    let second = rx.recv().await.expect("second event");

    assert!(matches!(first, EventMsg::ExecCommandBegin(_)));
    assert!(matches!(second, EventMsg::ExecCommandEnd(_)));
  }
}
