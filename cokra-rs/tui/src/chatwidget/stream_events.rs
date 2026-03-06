use super::*;
use cokra_protocol::AgentMessageContent;
use crate::history_cell::AgentMessageCell;
use crate::streaming::commit_tick::CommitTickScope;

impl ChatWidget {
  pub(super) fn on_agent_message_delta(&mut self, item_id: &str, delta: &str) {
    self
      .transcript
      .streamed_agent_item_ids
      .insert(item_id.to_string());
    let is_new = self.transcript.stream_controller.is_none();
    let controller = self
      .transcript
      .stream_controller
      .get_or_insert_with(|| crate::streaming::controller::StreamController::new(None));
    let _ = controller.push(delta);
    if is_new {
      self.app_event_tx.send(AppEvent::StartCommitAnimation);
    }
  }

  pub(super) fn on_agent_message(
    &mut self,
    item_id: &str,
    content: &[cokra_protocol::AgentMessageContent],
  ) {
    if self.transcript.streamed_agent_item_ids.contains(item_id) {
      return;
    }

    let mut lines = Vec::new();
    for part in content {
      match part {
        AgentMessageContent::Text { text } => lines.push(Line::from(text.clone())),
      }
    }
    if !lines.is_empty() {
      self.add_to_history(AgentMessageCell::new(lines, true));
    }
  }

  pub(super) fn on_plan_delta(&mut self, delta: &str) {
    let is_new = self.transcript.plan_stream_controller.is_none();
    let controller = self
      .transcript
      .plan_stream_controller
      .get_or_insert_with(|| crate::streaming::controller::PlanStreamController::new(None));
    let _ = controller.push(delta);
    if is_new {
      self.app_event_tx.send(AppEvent::StartCommitAnimation);
    }
  }

  pub(crate) fn on_commit_tick(&mut self) {
    let output = self
      .transcript
      .on_commit_tick(CommitTickScope::AnyMode, Instant::now());

    for cell in output.cells {
      self.add_boxed_history(cell);
    }

    if output.all_idle
      && !self.session.agent_turn_running
      && let Some(status) = self.bottom_pane.status_widget_mut()
    {
      status.pause_timer();
    }
  }
}
