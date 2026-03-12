use super::*;
use std::time::Duration;

use crate::exec_cell::ExecCall;
use crate::exec_cell::ExecCell;
use crate::exec_cell::model::CommandOutput;
use crate::exec_cell::new_active_exec_command;
use crate::history_cell::ApprovalRequestedHistoryCell;
use crate::history_cell::ExecHistoryCell;
use cokra_protocol::ExecApprovalRequestEvent;

impl ChatWidget {
  fn sync_exec_status_indicator(&mut self) {
    self.sync_status_indicator();
  }

  pub(super) fn handle_exec_begin_now(&mut self, event: &cokra_protocol::ExecCommandBeginEvent) {
    self.flush_answer_stream();

    let call = ExecCall {
      command_id: event.command_id.clone(),
      tool_name: event.tool_name.clone(),
      command: event.command.clone(),
      cwd: event.cwd.clone(),
      output: None,
      start_time: Some(Instant::now()),
      duration: None,
    };

    let merged_exec_cell = self
      .transcript
      .active_exec_cell
      .as_ref()
      .and_then(|cell| cell.as_any().downcast_ref::<ExecCell>())
      .and_then(|cell| cell.with_added_call(call.clone()));

    if let Some(merged_exec_cell) = merged_exec_cell {
      // ScrollbackFirst: before merging the new call, emit a snapshot of the
      // *current* completed state so the user sees the exploring list grow in
      // scrollback. We only emit when the current cell has no active calls
      // (all calls have output), which guarantees the snapshot shows
      // "● Explored" with no spinner residue.
      if self.stream_render_mode == StreamRenderMode::ScrollbackFirst {
        if let Some(cell) = self
          .transcript
          .active_exec_cell
          .as_ref()
          .and_then(|cell| cell.as_any().downcast_ref::<ExecCell>())
          .filter(|cell| !cell.is_active())
        {
          self
            .app_event_tx
            .insert_boxed_history_cell(Box::new(cell.scrollback_snapshot()));
        }
      }
      if let Some(cell) = self
        .transcript
        .active_exec_cell
        .as_mut()
        .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
      {
        *cell = merged_exec_cell;
      }
    } else {
      self.flush_active_exec_cell();
      self.transcript.active_exec_cell = Some(Box::new(new_active_exec_command(
        call.command_id.clone(),
        call.tool_name.clone(),
        call.command.clone(),
        call.cwd.clone(),
        self.animations_enabled(),
      )));
    }

    self
      .transcript
      .pending_exec_calls
      .insert(call.command_id.clone(), call);
    self.sync_exec_status_indicator();
    self.bump_active_cell_revision();
  }

  pub(super) fn on_exec_command_output_delta(
    &mut self,
    event: &cokra_protocol::ExecCommandOutputDeltaEvent,
  ) {
    if let Some(call) = self
      .transcript
      .pending_exec_calls
      .get_mut(&event.command_id)
    {
      let output = call.output.get_or_insert_with(CommandOutput::default);
      output.output.push_str(&event.output);
    }

    if let Some(cell) = self
      .transcript
      .active_exec_cell
      .as_mut()
      .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
      && cell.append_output(&event.command_id, &event.output)
    {
      self.bump_active_cell_revision();
    }
  }

  pub(super) fn handle_exec_approval_now(
    &mut self,
    ev: ExecApprovalRequestEvent,
  ) -> ChatWidgetAction {
    self.flush_answer_stream();
    // Use preserving variant: the exec cell (e.g. apply_patch) is still Running
    // and will be completed by the subsequent ExecCommandEnd event. Flushing it
    // here would strand a half-finished "Running" cell in the transcript.
    self.add_to_history_preserving_exec(ApprovalRequestedHistoryCell {
      command: ev.command.clone(),
    });
    ChatWidgetAction::ShowApproval(ev)
  }

  pub(super) fn handle_exec_end_now(&mut self, event: &cokra_protocol::ExecCommandEndEvent) {
    let mut call = self
      .transcript
      .pending_exec_calls
      .remove(&event.command_id)
      .unwrap_or(ExecCall {
        command_id: event.command_id.clone(),
        tool_name: "shell".to_string(),
        command: "<unknown>".to_string(),
        cwd: PathBuf::from("."),
        output: None,
        start_time: None,
        duration: None,
      });

    let mut output = call.output.unwrap_or_default();
    if !event.output.is_empty() {
      // Some producers stream output via deltas and then send a final end event
      // containing the full output snapshot. If we blindly append, we can duplicate
      // huge logs and make the UI unreadable.
      //
      // Heuristic: if the end payload is a full snapshot and starts with what we
      // already accumulated, replace instead of appending.
      // Tradeoff: this assumes snapshot outputs are prefix-preserving; if a runtime
      // sends a different kind of "end output", we fall back to append.
      if !output.output.is_empty()
        && event.output.len() > output.output.len()
        && event.output.starts_with(&output.output)
      {
        output.output = event.output.clone();
      } else {
        output.output.push_str(&event.output);
      }
    }
    output.exit_code = event.exit_code;

    let duration = call
      .start_time
      .map(|st| st.elapsed())
      .unwrap_or_else(|| Duration::from_millis(0));
    call.start_time = None;
    call.duration = Some(duration);
    call.output = Some(output.clone());

    let mut updated_active_exec = false;
    let mut should_flush_active = false;
    if let Some(cell) = self
      .transcript
      .active_exec_cell
      .as_mut()
      .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
    {
      cell.complete_call(&event.command_id, output, duration);
      updated_active_exec = true;
      should_flush_active = cell.should_flush();
    } else {
      self.add_to_history(ExecHistoryCell::from_exec_call(
        call,
        self.animations_enabled(),
      ));
    }
    if updated_active_exec {
      self.bump_active_cell_revision();
    }
    if should_flush_active {
      self.flush_active_exec_cell();
    }
    self.sync_exec_status_indicator();
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::app_event_sender::AppEventSender;
  use crate::tui::FrameRequester;
  use std::path::PathBuf;
  use tokio::sync::mpsc::unbounded_channel;

  fn make_widget() -> ChatWidget {
    let (tx, _rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut widget = ChatWidget::new(
      sender,
      FrameRequester::test_dummy(),
      false,
      StreamRenderMode::AnimatedPreview,
    );
    widget.set_agent_turn_running(true);
    widget
  }

  fn begin_event(
    command_id: &str,
    tool_name: &str,
    command: &str,
  ) -> cokra_protocol::ExecCommandBeginEvent {
    cokra_protocol::ExecCommandBeginEvent {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      command_id: command_id.to_string(),
      tool_name: tool_name.to_string(),
      command: command.to_string(),
      cwd: PathBuf::from("/tmp/project"),
    }
  }

  fn end_event(command_id: &str) -> cokra_protocol::ExecCommandEndEvent {
    cokra_protocol::ExecCommandEndEvent {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      command_id: command_id.to_string(),
      exit_code: 0,
      output: String::new(),
    }
  }

  fn delta_event(command_id: &str, output: &str) -> cokra_protocol::ExecCommandOutputDeltaEvent {
    cokra_protocol::ExecCommandOutputDeltaEvent {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      command_id: command_id.to_string(),
      output: output.to_string(),
    }
  }

  #[test]
  fn exec_end_restores_working_status_when_no_active_exec_calls_remain() {
    let mut widget = make_widget();

    widget.handle_exec_begin_now(&begin_event("call-1", "read_file", "read_file"));
    widget.handle_exec_end_now(&end_event("call-1"));

    let status = widget
      .bottom_pane
      .status_widget()
      .expect("status should remain visible while turn is active");
    assert_eq!(status.header(), "Working");
    assert_eq!(status.details(), None);
  }

  #[test]
  fn exec_end_switches_status_back_to_remaining_active_call() {
    let mut widget = make_widget();

    widget.handle_exec_begin_now(&begin_event("call-1", "list_dir", "list_dir"));
    widget.handle_exec_begin_now(&begin_event("call-2", "read_file", "read_file"));
    widget.handle_exec_end_now(&end_event("call-2"));

    let status = widget
      .bottom_pane
      .status_widget()
      .expect("status should remain visible while turn is active");
    assert_eq!(status.header(), "Running list_dir");
    assert_eq!(status.details(), Some("List_dir"));
  }

  #[test]
  fn exec_end_does_not_duplicate_output_when_end_contains_full_snapshot() {
    let mut widget = make_widget();

    widget.handle_exec_begin_now(&begin_event("call-1", "read_file", "read_file"));
    widget.on_exec_command_output_delta(&delta_event("call-1", "a\n"));

    // Simulate a producer that sends deltas and then includes the full snapshot in the end event.
    let end = cokra_protocol::ExecCommandEndEvent {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      command_id: "call-1".to_string(),
      exit_code: 0,
      output: "a\nb\n".to_string(),
    };
    widget.handle_exec_end_now(&end);

    let cell = widget
      .transcript
      .active_exec_cell
      .as_ref()
      .and_then(|c| c.as_any().downcast_ref::<ExecCell>())
      .expect("expected active exec cell");
    let call = cell.calls.first().expect("call");
    let out = call.output.as_ref().expect("output");
    assert_eq!(out.output, "a\nb\n");
  }

  #[test]
  fn exec_end_restores_reasoning_header_when_available() {
    let mut widget = make_widget();

    widget.handle_notice_event(&EventMsg::AgentReasoningDelta(
      cokra_protocol::AgentReasoningDeltaEvent {
        delta: "**Investigating rendering code** gathering files".to_string(),
      },
    ));
    widget.handle_exec_begin_now(&begin_event("call-1", "read_file", "read_file"));

    let status = widget
      .bottom_pane
      .status_widget()
      .expect("status should stay visible while exec runs");
    assert_eq!(status.header(), "Running read_file");

    widget.handle_exec_end_now(&end_event("call-1"));

    let status = widget
      .bottom_pane
      .status_widget()
      .expect("status should remain visible while turn is active");
    assert_eq!(status.header(), "Investigating rendering code");
    assert_eq!(status.details(), None);
  }

  #[test]
  fn sequential_exploring_calls_stay_in_one_exec_cell() {
    let mut widget = make_widget();

    widget.handle_exec_begin_now(&begin_event("call-1", "list_dir", "cokra-rs"));
    widget.handle_exec_end_now(&end_event("call-1"));
    widget.handle_exec_begin_now(&begin_event("call-2", "read_file", "PROJECT_STRUCTURE.md"));
    widget.handle_exec_end_now(&end_event("call-2"));
    widget.handle_exec_begin_now(&begin_event("call-3", "search_tool", "ExecCell"));
    widget.handle_exec_end_now(&end_event("call-3"));

    let cell = widget
      .transcript
      .active_exec_cell
      .as_ref()
      .and_then(|c| c.as_any().downcast_ref::<ExecCell>())
      .expect("expected grouped exploring exec cell");

    assert_eq!(cell.calls.len(), 3);
    assert!(cell.is_exploring_cell());

    let rendered = cell
      .display_lines(80)
      .iter()
      .map(Line::to_string)
      .collect::<Vec<_>>()
      .join("\n");
    // While the cell lives in active_exec_cell (turn still in progress) it
    // always shows "Exploring" with a spinner, regardless of whether
    // individual calls are finished. It becomes "Explored" only after
    // flush_active_cell() at turn end.
    assert!(
      rendered.contains("Exploring"),
      "expected Exploring while cell is live: {rendered}"
    );
    assert!(rendered.contains("List cokra-rs"));
    assert!(rendered.contains("Read PROJECT_STRUCTURE.md"));
    assert!(rendered.contains("Search ExecCell"));
  }

  #[test]
  fn sequential_code_search_calls_stay_in_one_exec_cell() {
    let mut widget = make_widget();

    widget.handle_exec_begin_now(&begin_event("call-1", "code_search", "agentteams"));
    widget.handle_exec_end_now(&end_event("call-1"));
    widget.handle_exec_begin_now(&begin_event("call-2", "code_search", "spawn_agent"));
    widget.handle_exec_end_now(&end_event("call-2"));

    let cell = widget
      .transcript
      .active_exec_cell
      .as_ref()
      .and_then(|c| c.as_any().downcast_ref::<ExecCell>())
      .expect("expected grouped code_search exec cell");

    assert_eq!(cell.calls.len(), 2);
    assert!(cell.is_exploring_cell());

    let rendered = cell
      .display_lines(80)
      .iter()
      .map(Line::to_string)
      .collect::<Vec<_>>()
      .join("\n");
    // Same as above: live cell always shows "Exploring" until flushed.
    assert!(
      rendered.contains("Exploring"),
      "expected Exploring while cell is live: {rendered}"
    );
    assert!(rendered.contains("Search agentteams"));
    assert!(rendered.contains("Search spawn_agent"));
  }

  fn make_scrollback_first_widget() -> (ChatWidget, tokio::sync::mpsc::UnboundedReceiver<AppEvent>)
  {
    let (tx, rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut widget = ChatWidget::new(
      sender,
      FrameRequester::test_dummy(),
      false,
      StreamRenderMode::ScrollbackFirst,
    );
    widget.set_agent_turn_running(true);
    (widget, rx)
  }

  #[test]
  fn scrollback_first_exploring_cell_writes_snapshot_on_each_new_call() {
    let (mut widget, mut rx) = make_scrollback_first_widget();

    // First exploring call — no snapshot yet (nothing to snapshot before it).
    widget.handle_exec_begin_now(&begin_event("call-1", "list_dir", "cokra-rs"));
    assert!(
      rx.try_recv().is_err(),
      "first exploring call should not emit a scrollback snapshot"
    );

    // Complete call-1: the cell is now fully idle (no active calls).
    widget.handle_exec_end_now(&end_event("call-1"));
    assert!(
      rx.try_recv().is_err(),
      "exec_end alone should not emit a snapshot"
    );

    // Second exploring call — active_exec_cell is now !is_active(), so a snapshot is emitted.
    widget.handle_exec_begin_now(&begin_event("call-2", "read_file", "Cargo.toml"));
    let snapshot_event = rx
      .try_recv()
      .expect("expected scrollback snapshot after second call");
    let AppEvent::InsertHistoryCell(snapshot) = snapshot_event else {
      panic!("expected InsertHistoryCell event");
    };
    assert!(
      snapshot.is_stream_continuation(),
      "scrollback snapshot should be marked as stream continuation to suppress blank line"
    );
    let rendered = snapshot
      .display_lines(80)
      .iter()
      .map(|l| l.to_string())
      .collect::<Vec<_>>()
      .join("\n");
    assert!(
      rendered.contains("Explored"),
      "snapshot should show Explored (no spinner): {rendered}"
    );
    assert!(
      rendered.contains("List cokra-rs"),
      "snapshot should contain the first call: {rendered}"
    );
    assert!(
      !rendered.contains("Cargo.toml"),
      "snapshot should NOT yet contain the second call: {rendered}"
    );

    // Complete call-2, then start call-3 — should emit another snapshot (now with 2 calls).
    widget.handle_exec_end_now(&end_event("call-2"));
    widget.handle_exec_begin_now(&begin_event("call-3", "glob", "**/*.rs"));
    let snapshot2_event = rx.try_recv().expect("expected second scrollback snapshot");
    let AppEvent::InsertHistoryCell(snapshot2) = snapshot2_event else {
      panic!("expected InsertHistoryCell event");
    };
    let rendered2 = snapshot2
      .display_lines(80)
      .iter()
      .map(|l| l.to_string())
      .collect::<Vec<_>>()
      .join("\n");
    assert!(
      rendered2.contains("Explored"),
      "second snapshot should show Explored (no spinner): {rendered2}"
    );
    assert!(
      rendered2.contains("List cokra-rs"),
      "second snapshot should contain call-1: {rendered2}"
    );
    assert!(
      rendered2.contains("Read Cargo.toml"),
      "second snapshot should contain call-2: {rendered2}"
    );
    assert!(
      !rendered2.contains("glob"),
      "second snapshot should NOT yet contain call-3: {rendered2}"
    );
  }

  #[test]
  fn animated_preview_exploring_cell_does_not_emit_intermediate_snapshots() {
    let mut widget = make_widget();

    widget.handle_exec_begin_now(&begin_event("call-1", "list_dir", "cokra-rs"));
    widget.handle_exec_begin_now(&begin_event("call-2", "read_file", "Cargo.toml"));

    // In AnimatedPreview mode no intermediate snapshots are written; the cell
    // is shown live in the viewport only.
    let cell = widget
      .transcript
      .active_exec_cell
      .as_ref()
      .and_then(|c| c.as_any().downcast_ref::<ExecCell>())
      .expect("expected active exec cell");
    assert_eq!(cell.calls.len(), 2);
  }

  #[test]
  fn agent_text_flushes_exec_cell_and_subsequent_exec_starts_new_cell() {
    let (tx, mut rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut widget = ChatWidget::new(
      sender,
      FrameRequester::test_dummy(),
      false,
      StreamRenderMode::AnimatedPreview,
    );
    widget.set_agent_turn_running(true);

    widget.handle_exec_begin_now(&begin_event("call-1", "list_dir", "cokra-rs"));
    // First agent delta should flush the exec cell (1:1 codex: handle_streaming_delta flushes active cell)
    widget.on_agent_message_delta("item-1", "I'll inspect the top-level layout.");

    // exec cell for call-1 should now be in history (flushed)
    let Some(AppEvent::InsertHistoryCell(exec_cell)) = rx.try_recv().ok() else {
      panic!("expected call-1 exec cell to be flushed to history when agent text starts");
    };
    let exec_rendered = exec_cell
      .display_lines(80)
      .iter()
      .map(Line::to_string)
      .collect::<Vec<_>>()
      .join("\n");
    assert!(exec_rendered.contains("List cokra-rs"));

    // After agent text flushes exec cell, a new exec begin starts a fresh exec cell
    widget.handle_exec_begin_now(&begin_event("call-2", "read_file", "Cargo.toml"));

    let cell = widget
      .transcript
      .active_exec_cell
      .as_ref()
      .and_then(|c| c.as_any().downcast_ref::<ExecCell>())
      .expect("expected new active exec cell for call-2");
    assert_eq!(
      cell.calls.len(),
      1,
      "call-2 should be in a fresh exec cell, not merged with call-1"
    );
  }
}
