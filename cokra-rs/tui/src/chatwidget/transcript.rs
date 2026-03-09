use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Instant;

use ratatui::text::Line;

use crate::app_event_sender::AppEventSender;
use crate::exec_cell::ExecCall;
use crate::history_cell::HistoryCell;
use crate::streaming::chunking::AdaptiveChunkingPolicy;
use crate::streaming::commit_tick::CommitTickOutput;
use crate::streaming::commit_tick::CommitTickScope;
use crate::streaming::commit_tick::run_commit_tick;
use crate::streaming::controller::PlanStreamController;
use crate::streaming::controller::StreamController;
use crate::xml_filter::XmlToolFilter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActiveCellTranscriptKey {
  pub(crate) revision: u64,
  pub(crate) is_stream_continuation: bool,
  pub(crate) animation_tick: Option<u64>,
}

pub(super) struct ActiveTranscriptState {
  pub(super) active_cell: Option<Box<dyn HistoryCell>>,
  pub(super) active_cell_revision: u64,
  pub(super) stream_controller: Option<StreamController>,
  pub(super) plan_stream_controller: Option<PlanStreamController>,
  pub(super) chunking_policy: AdaptiveChunkingPolicy,
  pub(super) pending_exec_calls: HashMap<String, ExecCall>,
  pub(super) streamed_agent_item_ids: HashSet<String>,
  pub(super) xml_tool_filter: Option<XmlToolFilter>,
  animations_enabled: bool,
}

impl ActiveTranscriptState {
  pub(super) fn new(animations_enabled: bool) -> Self {
    Self {
      active_cell: None,
      active_cell_revision: 0,
      stream_controller: None,
      plan_stream_controller: None,
      chunking_policy: AdaptiveChunkingPolicy::default(),
      pending_exec_calls: HashMap::new(),
      streamed_agent_item_ids: HashSet::new(),
      xml_tool_filter: None,
      animations_enabled,
    }
  }

  pub(super) fn animations_enabled(&self) -> bool {
    self.animations_enabled
  }

  pub(super) fn flush_active_cell(&mut self, app_event_tx: &AppEventSender) {
    if let Some(cell) = self.active_cell.take() {
      app_event_tx.insert_boxed_history_cell(cell);
    }
  }

  pub(super) fn flush_stream_controllers(
    &mut self,
    app_event_tx: &AppEventSender,
    wrap_width: Option<usize>,
  ) {
    if let Some(filter) = self.xml_tool_filter.as_mut() {
      let remaining = filter.flush();
      if !remaining.is_empty() {
        let controller = self
          .stream_controller
          .get_or_insert_with(|| StreamController::new(wrap_width));
        controller.set_width_if_uncommitted(wrap_width);
        let _ = controller.push(&remaining);
      }
    }

    if let Some(mut controller) = self.stream_controller.take()
      && let Some(cell) = controller.finalize()
    {
      self.flush_active_cell(app_event_tx);
      app_event_tx.insert_boxed_history_cell(cell);
    }
    if let Some(mut controller) = self.plan_stream_controller.take()
      && let Some(cell) = controller.finalize()
    {
      self.flush_active_cell(app_event_tx);
      app_event_tx.insert_boxed_history_cell(cell);
    }
  }

  pub(super) fn bump_active_cell_revision(&mut self) {
    self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
  }

  pub(super) fn on_commit_tick(
    &mut self,
    scope: CommitTickScope,
    now: Instant,
  ) -> CommitTickOutput {
    run_commit_tick(
      &mut self.chunking_policy,
      self.stream_controller.as_mut(),
      self.plan_stream_controller.as_mut(),
      scope,
      now,
    )
  }

  pub(super) fn clear_turn_state(&mut self) {
    self.stream_controller = None;
    self.plan_stream_controller = None;
    self.streamed_agent_item_ids.clear();
    self.xml_tool_filter = None;
  }

  pub(super) fn clear_exec_state(&mut self) {
    self.pending_exec_calls.clear();
  }

  pub(crate) fn active_cell_transcript_key(&self) -> Option<ActiveCellTranscriptKey> {
    let cell = self.active_cell.as_ref()?;
    Some(ActiveCellTranscriptKey {
      revision: self.active_cell_revision,
      is_stream_continuation: cell.is_stream_continuation(),
      animation_tick: cell.transcript_animation_tick(),
    })
  }

  pub(crate) fn active_cell_transcript_lines(&self, width: u16) -> Option<Vec<Line<'static>>> {
    let cell = self.active_cell.as_ref()?;
    let lines = cell.transcript_lines(width.max(1));
    (!lines.is_empty()).then_some(lines)
  }

  pub(super) fn compose_alt_screen_lines(
    &self,
    history_lines: &[Line<'static>],
    active_tail_lines: &[Line<'static>],
  ) -> Vec<Line<'static>> {
    let mut lines = history_lines.to_vec();
    if !active_tail_lines.is_empty() {
      if !lines.is_empty() {
        lines.push(Line::from(""));
      }
      lines.extend(active_tail_lines.iter().cloned());
    }
    lines
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::history_cell::PlainHistoryCell;
  use ratatui::text::Line;

  #[test]
  fn active_cell_transcript_cache_key_tracks_revision() {
    let mut state = ActiveTranscriptState::new(true);
    state.active_cell = Some(Box::new(PlainHistoryCell::new(vec![Line::from("hello")])));

    let first = state.active_cell_transcript_key().expect("key");
    assert_eq!(first.revision, 0);
    assert!(!first.is_stream_continuation);
    assert_eq!(
      state.active_cell_transcript_lines(80),
      Some(vec![Line::from("hello")])
    );

    state.bump_active_cell_revision();
    let second = state.active_cell_transcript_key().expect("key");
    assert_eq!(second.revision, 1);
  }

  #[test]
  fn compose_alt_screen_lines_appends_live_tail_with_separator() {
    let mut state = ActiveTranscriptState::new(true);
    state.active_cell = Some(Box::new(PlainHistoryCell::new(vec![Line::from("tail")])));

    let lines = state.compose_alt_screen_lines(&[Line::from("history")], &[Line::from("tail")]);
    assert_eq!(
      lines,
      vec![Line::from("history"), Line::from(""), Line::from("tail")]
    );
  }
}
