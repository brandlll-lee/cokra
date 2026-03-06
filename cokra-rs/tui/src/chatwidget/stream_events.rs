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
    let committed = controller.push(delta);
    if is_new || committed {
      self.app_event_tx.send(AppEvent::StartCommitAnimation);
    }
    if committed {
      self.run_catch_up_commit_tick();
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
    let committed = controller.push(delta);
    if is_new || committed {
      self.app_event_tx.send(AppEvent::StartCommitAnimation);
    }
    if committed {
      self.run_catch_up_commit_tick();
    }
  }

  pub(crate) fn on_commit_tick(&mut self) {
    self.run_commit_tick_with_scope(CommitTickScope::AnyMode);
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::app_event::AppEvent;
  use crate::app_event_sender::AppEventSender;
  use crate::tui::FrameRequester;
  use tokio::sync::mpsc::unbounded_channel;

  #[test]
  fn newline_delta_starts_commit_animation_immediately() {
    let (tx, mut rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut widget = ChatWidget::new(sender, FrameRequester::test_dummy(), false);

    widget.on_agent_message_delta("item-1", "hello\n");

    let mut saw_start = false;
    while let Ok(event) = rx.try_recv() {
      if matches!(event, AppEvent::StartCommitAnimation) {
        saw_start = true;
      }
    }

    assert!(saw_start);
  }
}
