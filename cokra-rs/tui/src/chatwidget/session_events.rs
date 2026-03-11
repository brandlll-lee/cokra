use super::*;
use crate::history_cell::TurnCompleteHistoryCell;
use crate::history_cell::new_session_info;

impl ChatWidget {
  pub(super) fn on_session_configured(&mut self, event: &cokra_protocol::SessionConfiguredEvent) {
    let is_first = !self.session.has_seen_session_configured;
    self.session.has_seen_session_configured = true;
    self.session.set_model_name(event.model.clone());
    if !is_first {
      self.add_to_history(new_session_info(
        event.model.clone(),
        event.approval_policy.clone(),
        event.sandbox_mode.clone(),
        None,
        false,
      ));
    }
  }

  pub(super) fn on_turn_started(&mut self, event: &cokra_protocol::TurnStartedEvent) {
    self.session.reset_turn_status();
    self.set_agent_turn_running(true);
    self.session.cwd = Some(event.cwd.clone());
  }

  pub(super) fn on_token_count(&mut self, event: &cokra_protocol::TokenCountEvent) {
    self.session.token_usage.input_tokens = event.input_tokens;
    self.session.token_usage.output_tokens = event.output_tokens;
    self.session.token_usage.total_tokens = event.total_tokens;
  }

  pub(super) fn on_error(&mut self, event: &cokra_protocol::ErrorEvent) {
    self.app_event_tx.send(AppEvent::StopCommitAnimation);
    self.flush_stream_controllers();
    self.flush_active_cell();
    self.transcript.clear_exec_state();
    self.transcript.clear_turn_state();
    self.session.reset_turn_status();
    self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
      Span::from("error: ").red(),
      Span::from(event.user_facing_message.clone()),
    ])]));
  }

  pub(super) fn on_turn_complete(&mut self, event: &cokra_protocol::TurnCompleteEvent) {
    self.flush_stream_controllers();
    self.flush_active_cell();

    let elapsed_seconds = self
      .bottom_pane
      .status_widget()
      .map(|status| self.session.worked_elapsed_from(status.elapsed_seconds()));

    self.app_event_tx.send(AppEvent::StopCommitAnimation);
    if matches!(event.status, cokra_protocol::CompletionStatus::Success) {
      self.add_to_history(TurnCompleteHistoryCell {
        elapsed_seconds,
        input_tokens: self.session.token_usage.input_tokens,
        output_tokens: self.session.token_usage.output_tokens,
      });
    }
    self.session.reset_turn_status();
    self.set_agent_turn_running(false);
    self.transcript.clear_exec_state();
    self.transcript.clear_turn_state();
  }

  pub(super) fn on_turn_aborted(&mut self, event: &cokra_protocol::TurnAbortedEvent) {
    self.app_event_tx.send(AppEvent::StopCommitAnimation);
    self.flush_stream_controllers();
    self.flush_active_cell();
    self.transcript.clear_exec_state();
    self.transcript.clear_turn_state();
    self.session.reset_turn_status();
    self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
      Span::from("aborted: ").yellow(),
      Span::from(event.reason.clone()),
    ])]));
    self.set_agent_turn_running(false);
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::app_event::AppEvent;
  use crate::app_event_sender::AppEventSender;
  use crate::tui::FrameRequester;
  use tokio::sync::mpsc::unbounded_channel;

  fn configured_event(model: &str) -> cokra_protocol::SessionConfiguredEvent {
    cokra_protocol::SessionConfiguredEvent {
      thread_id: "thread-1".to_string(),
      model: model.to_string(),
      approval_policy: "Ask".to_string(),
      sandbox_mode: "Permissive".to_string(),
    }
  }

  #[test]
  fn first_session_config_only_updates_state_without_history_duplication() {
    let (tx, mut rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut widget = ChatWidget::new(
      sender,
      FrameRequester::test_dummy(),
      false,
      StreamRenderMode::AnimatedPreview,
    );

    widget.on_session_configured(&configured_event("openai/gpt-5.2-codex"));

    assert_eq!(widget.model_name(), "openai/gpt-5.2-codex");
    assert!(
      rx.try_recv().is_err(),
      "first SessionConfigured must not enqueue a duplicate session header"
    );
  }

  #[test]
  fn later_session_config_inserts_compact_session_header() {
    let (tx, mut rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut widget = ChatWidget::new(
      sender,
      FrameRequester::test_dummy(),
      false,
      StreamRenderMode::AnimatedPreview,
    );

    widget.on_session_configured(&configured_event("openai/gpt-5.2-codex"));
    widget.on_session_configured(&configured_event("github/claude-sonnet-4.6"));

    let Some(AppEvent::InsertHistoryCell(cell)) = rx.try_recv().ok() else {
      panic!("second SessionConfigured should enqueue a history cell");
    };
    let rendered = cell
      .display_lines(80)
      .into_iter()
      .map(|line| {
        line
          .spans
          .into_iter()
          .map(|span| span.content)
          .collect::<String>()
      })
      .collect::<Vec<_>>()
      .join("\n");

    assert!(rendered.contains("┌─ cokra ─ github/claude-sonnet-4.6"));
    assert!(!rendered.contains("Welcome to Cokra"));
  }
}
