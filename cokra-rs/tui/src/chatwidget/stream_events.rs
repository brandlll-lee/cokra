use super::*;
use crate::history_cell::AgentMessageCell;
use crate::streaming::commit_tick::CommitTickScope;
use crate::xml_filter::XmlToolFilter;
use crate::xml_filter::strip_inline_xml_tool_tags;
use cokra_protocol::AgentMessageContent;

impl ChatWidget {
  pub(super) fn on_agent_message_delta(&mut self, item_id: &str, delta: &str) {
    let xml_filter = self
      .transcript
      .xml_tool_filter
      .get_or_insert_with(XmlToolFilter::new);
    let filtered_delta = xml_filter.filter(delta);
    if filtered_delta.is_empty() {
      return;
    }

    self
      .transcript
      .streamed_agent_item_ids
      .insert(item_id.to_string());
    let is_new = self.transcript.stream_controller.is_none();
    let wrap_width = self.streaming_wrap_width();
    let controller = self
      .transcript
      .stream_controller
      .get_or_insert_with(|| crate::streaming::controller::StreamController::new(wrap_width));
    controller.set_width_if_uncommitted(wrap_width);
    let _ = controller.push(&filtered_delta);
    self.refresh_streaming_agent_preview();
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
        AgentMessageContent::Text { text } => {
          let filtered = strip_inline_xml_tool_tags(text);
          if !filtered.trim().is_empty() {
            lines.push(Line::from(filtered));
          }
        }
      }
    }
    if !lines.is_empty() {
      self.add_to_history(AgentMessageCell::new(lines, true));
    }
  }

  pub(super) fn on_plan_delta(&mut self, delta: &str) {
    let is_new = self.transcript.plan_stream_controller.is_none();
    let wrap_width = self.streaming_wrap_width();
    let controller = self
      .transcript
      .plan_stream_controller
      .get_or_insert_with(|| crate::streaming::controller::PlanStreamController::new(wrap_width));
    controller.set_width_if_uncommitted(wrap_width);
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
  use crate::app_event_sender::AppEventSender;
  use crate::tui::FrameRequester;
  use tokio::sync::mpsc::unbounded_channel;

  #[test]
  fn agent_delta_updates_active_preview_immediately() {
    let (tx, mut rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut widget = ChatWidget::new(sender, FrameRequester::test_dummy(), false);

    widget.on_agent_message_delta("item-1", "| A | B |\n| --- | --- |\n| 1 | 2 |");

    let lines = widget
      .active_cell_transcript_lines(80)
      .expect("stream preview should exist");
    let rendered = lines
      .iter()
      .map(|line| {
        line
          .spans
          .iter()
          .map(|span| span.content.clone())
          .collect::<String>()
      })
      .collect::<Vec<_>>()
      .join("\n");
    assert!(rendered.contains("┌"));
    assert!(rendered.contains("│ 1"));
    assert!(
      rx.try_recv().is_ok(),
      "stream start should still trigger UI wake-up"
    );
  }
}
