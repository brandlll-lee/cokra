use super::*;
use crate::history_cell::TurnCompleteHistoryCell;
use crate::history_cell::new_session_info;

impl ChatWidget {
  pub(super) fn on_session_configured(&mut self, event: &cokra_protocol::SessionConfiguredEvent) {
    let is_first = !self.session.has_seen_session_configured;
    self.session.has_seen_session_configured = true;
    self.session.set_model_name(event.model.clone());
    self.add_to_history(new_session_info(
      event.model.clone(),
      event.approval_policy.clone(),
      event.sandbox_mode.clone(),
      None,
      is_first,
    ));
  }

  pub(super) fn on_turn_started(&mut self, event: &cokra_protocol::TurnStartedEvent) {
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
    self.transcript.clear_exec_state();
    self.transcript.clear_turn_state();
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
    self.set_agent_turn_running(false);
    self.transcript.clear_exec_state();
    self.transcript.streamed_agent_item_ids.clear();
  }

  pub(super) fn on_turn_aborted(&mut self, event: &cokra_protocol::TurnAbortedEvent) {
    self.app_event_tx.send(AppEvent::StopCommitAnimation);
    self.transcript.clear_exec_state();
    self.transcript.clear_turn_state();
    self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
      Span::from("aborted: ").yellow(),
      Span::from(event.reason.clone()),
    ])]));
    self.set_agent_turn_running(false);
  }
}
