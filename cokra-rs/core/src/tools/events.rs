use std::path::Path;

use cokra_protocol::{EventMsg, ExecCommandBeginEvent, ExecCommandEndEvent};
use tokio::sync::mpsc;

use crate::session::Session;
use crate::tools::context::{FunctionCallError, ToolOutput};

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
}

impl ToolEmitter {
  pub fn new(tool_name: impl Into<String>) -> Self {
    Self {
      tool_name: tool_name.into(),
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
            command: self.tool_name.clone(),
            cwd: ctx.cwd.to_path_buf(),
          }),
        )
        .await;
      }
      ToolEventStage::Success(output) => {
        emit_event(
          &ctx,
          EventMsg::ExecCommandEnd(ExecCommandEndEvent {
            thread_id: ctx.thread_id.to_string(),
            turn_id: ctx.turn_id.to_string(),
            command_id: ctx.call_id.to_string(),
            exit_code: if output.is_error { 1 } else { 0 },
            output: output.content,
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
        let content = output.content.clone();
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
      .finish(
        ctx,
        Ok(ToolOutput {
          id: "call-1".to_string(),
          content: "ok".to_string(),
          is_error: false,
        }),
      )
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

async fn emit_event(ctx: &ToolEventCtx<'_>, event: EventMsg) {
  ctx.session.emit_event(event.clone());
  if let Some(tx_event) = &ctx.tx_event {
    let _ = tx_event.send(event).await;
  }
}
