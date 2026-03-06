use super::*;
use std::time::Duration;

use crate::exec_cell::ExecCall;
use crate::exec_cell::ExecCell;
use crate::exec_cell::model::CommandOutput;
use crate::exec_cell::new_active_exec_command;
use crate::history_cell::ExecHistoryCell;

impl ChatWidget {
  pub(super) fn on_exec_command_begin(
    &mut self,
    event: &cokra_protocol::ExecCommandBeginEvent,
  ) {
    self.flush_stream_controllers();
    if let Some(status) = self.bottom_pane.status_widget_mut() {
      status.update_header(format!("Running {}", event.tool_name));
      status.update_details(Some(event.command.clone()));
    }

    let call = ExecCall {
      command_id: event.command_id.clone(),
      tool_name: event.tool_name.clone(),
      command: event.command.clone(),
      cwd: event.cwd.clone(),
      output: None,
      start_time: Some(Instant::now()),
      duration: None,
    };

    let reuse_exec_cell = self
      .transcript
      .active_cell
      .as_ref()
      .and_then(|cell| cell.as_any().downcast_ref::<ExecCell>())
      .is_some();

    if reuse_exec_cell {
      if let Some(cell) = self
        .transcript
        .active_cell
        .as_mut()
        .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
      {
        cell.push_call(call.clone());
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
    self.bump_active_cell_revision();
  }

  pub(super) fn on_exec_command_output_delta(
    &mut self,
    event: &cokra_protocol::ExecCommandOutputDeltaEvent,
  ) {
    if let Some(call) = self.transcript.pending_exec_calls.get_mut(&event.command_id) {
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
      should_flush_active = !cell.is_active();
    } else {
      self.add_to_history(ExecHistoryCell::from_exec_call(call, self.animations_enabled()));
    }
    if updated_active_exec {
      self.bump_active_cell_revision();
    }
    if should_flush_active {
      self.flush_active_cell();
    }
  }
}
