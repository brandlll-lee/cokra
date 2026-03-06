use super::*;
use std::time::Duration;

use crate::exec_cell::ExecCall;
use crate::exec_cell::ExecCell;
use crate::exec_cell::model::CommandOutput;
use crate::exec_cell::new_active_exec_command;
use crate::history_cell::ExecHistoryCell;

impl ChatWidget {
  fn sync_exec_status_indicator(&mut self) {
    let Some(status) = self.bottom_pane.status_widget_mut() else {
      return;
    };

    let active_exec_call = self
      .transcript
      .active_cell
      .as_ref()
      .and_then(|cell| cell.as_any().downcast_ref::<ExecCell>())
      .and_then(ExecCell::active_call);

    if let Some(call) = active_exec_call {
      status.update_header(format!("Running {}", call.tool_name));
      status.update_details(Some(call.command.clone()));
    } else if self.session.agent_turn_running {
      status.update_header("Working".to_string());
      status.update_details(None);
    }
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
      .active_cell
      .as_ref()
      .and_then(|cell| cell.as_any().downcast_ref::<ExecCell>())
      .and_then(|cell| cell.with_added_call(call.clone()));

    if let Some(merged_exec_cell) = merged_exec_cell {
      if let Some(cell) = self
        .transcript
        .active_cell
        .as_mut()
        .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
      {
        *cell = merged_exec_cell;
      }
    } else {
      self.flush_active_cell();
      self.transcript.active_cell = Some(Box::new(new_active_exec_command(
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
      .active_cell
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
      output.output.push_str(&event.output);
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
      .active_cell
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
      self.flush_active_cell();
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
    let mut widget = ChatWidget::new(sender, FrameRequester::test_dummy(), false);
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
}
