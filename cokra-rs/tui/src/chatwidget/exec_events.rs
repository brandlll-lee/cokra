use super::*;
use std::time::Duration;

use crate::exec_cell::ExecCall;
use crate::exec_cell::ExecCell;
use crate::exec_cell::model::CommandOutput;
use crate::exec_cell::new_active_exec_command;
use crate::history_cell::ExecHistoryCell;

impl ChatWidget {
  fn sync_exec_status_indicator(&mut self) {
    self.sync_status_indicator();
  }

  pub(super) fn on_exec_command_begin(&mut self, event: &cokra_protocol::ExecCommandBeginEvent) {
    self.flush_stream_controllers();

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

  pub(super) fn on_exec_command_end(&mut self, event: &cokra_protocol::ExecCommandEndEvent) {
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

    widget.on_exec_command_begin(&begin_event("call-1", "read_file", "read_file"));
    widget.on_exec_command_end(&end_event("call-1"));

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

    widget.on_exec_command_begin(&begin_event("call-1", "list_dir", "list_dir"));
    widget.on_exec_command_begin(&begin_event("call-2", "read_file", "read_file"));
    widget.on_exec_command_end(&end_event("call-2"));

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

    widget.on_exec_command_begin(&begin_event("call-1", "read_file", "read_file"));
    widget.on_exec_command_output_delta(&delta_event("call-1", "a\n"));

    // Simulate a producer that sends deltas and then includes the full snapshot in the end event.
    let end = cokra_protocol::ExecCommandEndEvent {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      command_id: "call-1".to_string(),
      exit_code: 0,
      output: "a\nb\n".to_string(),
    };
    widget.on_exec_command_end(&end);

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
    widget.on_exec_command_begin(&begin_event("call-1", "read_file", "read_file"));

    let status = widget
      .bottom_pane
      .status_widget()
      .expect("status should stay visible while exec runs");
    assert_eq!(status.header(), "Running read_file");

    widget.on_exec_command_end(&end_event("call-1"));

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

    widget.on_exec_command_begin(&begin_event("call-1", "list_dir", "cokra-rs"));
    widget.on_exec_command_end(&end_event("call-1"));
    widget.on_exec_command_begin(&begin_event("call-2", "read_file", "PROJECT_STRUCTURE.md"));
    widget.on_exec_command_end(&end_event("call-2"));
    widget.on_exec_command_begin(&begin_event("call-3", "search_tool", "ExecCell"));
    widget.on_exec_command_end(&end_event("call-3"));

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
    assert!(rendered.contains("Explored"));
    assert!(rendered.contains("List cokra-rs"));
    assert!(rendered.contains("Read PROJECT_STRUCTURE.md"));
    assert!(rendered.contains("Search ExecCell"));
  }

  #[test]
  fn agent_preview_does_not_replace_active_exec_cell() {
    let mut widget = make_widget();

    widget.on_exec_command_begin(&begin_event("call-1", "list_dir", "cokra-rs"));
    widget.on_agent_message_delta("item-1", "I'll inspect the top-level layout.");
    widget.on_exec_command_begin(&begin_event("call-2", "read_file", "Cargo.toml"));

    let cell = widget
      .transcript
      .active_exec_cell
      .as_ref()
      .and_then(|c| c.as_any().downcast_ref::<ExecCell>())
      .expect("expected active exec cell");
    assert_eq!(cell.calls.len(), 2);

    let lines = widget
      .active_cell_transcript_lines(80)
      .expect("expected combined live transcript");
    let rendered = lines.iter().map(Line::to_string).collect::<Vec<_>>().join("\n");
    assert!(rendered.contains("I'll inspect the top-level layout."));
    assert!(rendered.contains("List cokra-rs"));
    assert!(rendered.contains("Read Cargo.toml"));
  }
}
