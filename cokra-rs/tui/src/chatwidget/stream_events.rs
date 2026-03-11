use super::*;
use crate::history_cell::AgentMessageCell;
use crate::streaming::commit_tick::CommitTickScope;
use crate::xml_filter::XmlToolFilter;
use crate::xml_filter::strip_inline_xml_tool_tags;
use cokra_protocol::AgentMessageContent;

impl ChatWidget {
  /// Called when a stream finishes (finalized or a non-stream message arrives).
  /// Flushes any deferred interrupts that were queued while the stream was active.
  fn handle_stream_finished(&mut self) -> Option<ChatWidgetAction> {
    self.flush_interrupt_queue()
  }

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
    if is_new {
      self.flush_active_exec_cell();
    }
    let controller = self
      .transcript
      .stream_controller
      .get_or_insert_with(|| crate::streaming::controller::StreamController::new(wrap_width));
    controller.set_width_if_uncommitted(wrap_width);
    let committed = controller.push(&filtered_delta);
    if self.stream_render_mode == StreamRenderMode::ScrollbackFirst
      && committed
      && let Some(cell) = controller.drain_committed_now()
    {
      self.append_boxed_history(cell);
    }
    self.refresh_streaming_agent_preview();
    if self.stream_render_mode == StreamRenderMode::AnimatedPreview && is_new {
      self.app_event_tx.send(AppEvent::StartCommitAnimation);
    }
  }

  pub(super) fn on_agent_message(
    &mut self,
    item_id: &str,
    content: &[cokra_protocol::AgentMessageContent],
  ) -> Option<ChatWidgetAction> {
    if self.transcript.streamed_agent_item_ids.contains(item_id) {
      // Content was already streamed via deltas; finalize the stream.
      self.flush_answer_stream();
      return self.handle_stream_finished();
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
      self.flush_answer_stream();
      self.add_to_history(AgentMessageCell::new(lines, true));
    }
    self.handle_stream_finished()
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
    match self.stream_render_mode {
      StreamRenderMode::AnimatedPreview => {
        if is_new || committed {
          self.app_event_tx.send(AppEvent::StartCommitAnimation);
        }
        if committed {
          self.run_catch_up_commit_tick();
        }
      }
      StreamRenderMode::ScrollbackFirst => {
        // Tradeoff: proposed plans now favor terminal-native history in inline mode,
        // even though that gives up the old line-by-line commit animation there.
        if committed && let Some(cell) = controller.drain_committed_now() {
          self.append_boxed_history(cell);
        }
      }
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
  use std::path::PathBuf;
  use tokio::sync::mpsc::unbounded_channel;

  #[test]
  fn animated_preview_flushes_exec_cell_before_agent_text() {
    let (tx, mut rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut widget = ChatWidget::new(
      sender,
      FrameRequester::test_dummy(),
      false,
      StreamRenderMode::AnimatedPreview,
    );

    widget.handle_exec_begin_now(&cokra_protocol::ExecCommandBeginEvent {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      command_id: "call-1".to_string(),
      tool_name: "read_file".to_string(),
      command: "src/main.rs".to_string(),
      cwd: PathBuf::from("/tmp/project"),
    });
    widget.handle_exec_end_now(&cokra_protocol::ExecCommandEndEvent {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      command_id: "call-1".to_string(),
      exit_code: 0,
      output: String::new(),
    });

    // Before agent text starts, exec cell should still be in active state (not flushed yet)
    assert!(
      widget.transcript.active_exec_cell.is_some(),
      "exec cell should be active before agent text arrives"
    );

    // First agent delta should trigger flush of exec cell to history
    widget.on_agent_message_delta("item-1", "Here is the answer");

    let Some(AppEvent::InsertHistoryCell(exec_cell)) = rx.try_recv().ok() else {
      panic!("expected exec cell to flush to history when agent text starts");
    };
    let exec_rendered = exec_cell
      .display_lines(80)
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
    assert!(exec_rendered.contains("Read src/main.rs"));

    // Exec cell should no longer be active
    assert!(
      widget.transcript.active_exec_cell.is_none(),
      "exec cell should be flushed after agent text starts"
    );

    // Subsequent deltas should not flush again (only commit animation events are allowed)
    widget.on_agent_message_delta("item-1", " and more text");
    while let Ok(event) = rx.try_recv() {
      assert!(
        matches!(event, AppEvent::StartCommitAnimation),
        "subsequent deltas should only emit StartCommitAnimation, got {event:?}"
      );
    }
  }

  #[test]
  fn agent_delta_updates_active_preview_immediately() {
    let (tx, mut rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut widget = ChatWidget::new(
      sender,
      FrameRequester::test_dummy(),
      false,
      StreamRenderMode::AnimatedPreview,
    );

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

  #[test]
  fn scrollback_first_commits_completed_lines_without_growing_preview() {
    let (tx, mut rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut widget = ChatWidget::new(
      sender,
      FrameRequester::test_dummy(),
      false,
      StreamRenderMode::ScrollbackFirst,
    );

    widget.on_agent_message_delta("item-1", "hello\nworld");

    let Some(AppEvent::InsertHistoryCell(cell)) = rx.try_recv().ok() else {
      panic!("completed stream line should be inserted into history immediately");
    };
    let rendered = cell
      .display_lines(80)
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
    assert!(rendered.contains("hello"));

    let tail_rendered = widget
      .active_cell_transcript_lines(80)
      .expect("uncommitted tail should stay in the active preview")
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
    assert!(
      tail_rendered.contains("world"),
      "active preview should only retain the uncommitted tail"
    );
    assert!(
      !tail_rendered.contains("hello"),
      "completed lines should leave the preview once committed to scrollback"
    );
    assert!(
      rx.try_recv().is_err(),
      "scrollback-first mode should not start commit animation for agent text"
    );
  }

  #[test]
  fn scrollback_first_flushes_active_exec_before_agent_history_chunks() {
    let (tx, mut rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut widget = ChatWidget::new(
      sender,
      FrameRequester::test_dummy(),
      false,
      StreamRenderMode::ScrollbackFirst,
    );

    widget.handle_exec_begin_now(&cokra_protocol::ExecCommandBeginEvent {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      command_id: "call-1".to_string(),
      tool_name: "list_dir".to_string(),
      command: "core/src".to_string(),
      cwd: PathBuf::from("/tmp/project"),
    });
    widget.handle_exec_end_now(&cokra_protocol::ExecCommandEndEvent {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      command_id: "call-1".to_string(),
      exit_code: 0,
      output: String::new(),
    });
    widget.on_agent_message_delta("item-1", "answer line\nnext line");

    let Some(AppEvent::InsertHistoryCell(exec_cell)) = rx.try_recv().ok() else {
      panic!("expected exec cell to flush before agent text");
    };
    let exec_rendered = exec_cell
      .display_lines(80)
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
    assert!(exec_rendered.contains("List core/src"));

    let Some(AppEvent::InsertHistoryCell(agent_cell)) = rx.try_recv().ok() else {
      panic!("expected committed agent text after exec cell");
    };
    let agent_rendered = agent_cell
      .display_lines(80)
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
    assert!(agent_rendered.contains("answer line"));
  }
}
