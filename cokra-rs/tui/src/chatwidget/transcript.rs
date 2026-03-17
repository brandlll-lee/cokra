use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Instant;

use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use crate::app_event_sender::AppEventSender;
use crate::exec_cell::ExecCall;
use crate::history_cell::HistoryCell;
use crate::history_cell::PlainHistoryCell;
use crate::history_cell::TodoUpdateCell;
use crate::terminal_palette::light_blue;
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

/// Accumulates exploring call data across flushed ExecCells within a single turn.
/// Drives the live "● Explored: read N files, searched for M patterns" summary.
#[derive(Debug, Default)]
pub(super) struct ExploringAccumulator {
  /// (tool_name, command) pairs from absorbed exploring cells.
  calls: Vec<(String, String)>,
}

impl ExploringAccumulator {
  /// Absorb all calls from a completed exploring ExecCell.
  pub(super) fn absorb(&mut self, cell: &crate::exec_cell::ExecCell) {
    for call in cell.iter_calls() {
      self.calls.push((call.tool_name.clone(), call.command.clone()));
    }
  }

  pub(super) fn is_empty(&self) -> bool {
    self.calls.is_empty()
  }

  /// Count completed calls by category.
  pub(super) fn counts(&self) -> ExploringSummaryCounts {
    let mut c = ExploringSummaryCounts::default();
    for (tool, _) in &self.calls {
      c.add(tool);
    }
    c
  }

  /// Take the accumulated data and produce a summary history cell for scrollback.
  pub(super) fn take_summary_cell(&mut self) -> Option<PlainHistoryCell> {
    if self.calls.is_empty() {
      return None;
    }
    let counts = self.counts();
    self.calls.clear();
    Some(PlainHistoryCell::new(vec![counts.to_summary_line()]))
  }

  pub(super) fn clear(&mut self) {
    self.calls.clear();
  }
}

#[derive(Debug, Default)]
pub(super) struct ExploringSummaryCounts {
  pub(super) reads: usize,
  pub(super) searches: usize,
  pub(super) lists: usize,
}

impl ExploringSummaryCounts {
  pub(super) fn add(&mut self, tool_name: &str) {
    match tool_name {
      "read_file" | "read_many_files" => self.reads += 1,
      "grep_files" | "search_tool" | "code_search" => self.searches += 1,
      "list_dir" | "glob" => self.lists += 1,
      _ => {}
    }
  }

  pub(super) fn merge(&mut self, other: &Self) {
    self.reads += other.reads;
    self.searches += other.searches;
    self.lists += other.lists;
  }

  pub(super) fn is_empty(&self) -> bool {
    self.reads == 0 && self.searches == 0 && self.lists == 0
  }

  pub(super) fn to_summary_line(&self) -> Line<'static> {
    let mut parts = Vec::new();
    if self.reads > 0 {
      parts.push(format!(
        "read {} file{}",
        self.reads,
        if self.reads == 1 { "" } else { "s" }
      ));
    }
    if self.searches > 0 {
      parts.push(format!(
        "searched for {} pattern{}",
        self.searches,
        if self.searches == 1 { "" } else { "s" }
      ));
    }
    if self.lists > 0 {
      parts.push(format!(
        "listed {} dir{}",
        self.lists,
        if self.lists == 1 { "" } else { "s" }
      ));
    }
    let text = if parts.is_empty() {
      "Explored".to_string()
    } else {
      parts.join(", ")
    };
    Line::from(vec![
      Span::from("● ").dim(),
      Span::from("Explored").style(
        ratatui::style::Style::new()
          .fg(light_blue())
          .add_modifier(ratatui::style::Modifier::BOLD),
      ),
      Span::from(format!(" {text}")).dim(),
    ])
  }
}

pub(super) struct ActiveTranscriptState {
  pub(super) active_collab_summary: Option<Box<dyn HistoryCell>>,
  pub(super) active_exec_cell: Option<Box<dyn HistoryCell>>,
  pub(super) active_agent_preview: Option<Box<dyn HistoryCell>>,
  /// Live todo widget. Uses concrete type (not Box<dyn HistoryCell>) because
  /// this slot only ever holds a TodoUpdateCell — no dynamic dispatch needed.
  pub(super) active_todo: Option<TodoUpdateCell>,
  /// Accumulates exploring call data across flushed ExecCells within a turn.
  pub(super) exploring_accumulator: ExploringAccumulator,
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
      active_collab_summary: None,
      active_exec_cell: None,
      active_agent_preview: None,
      active_todo: None,
      exploring_accumulator: ExploringAccumulator::default(),
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

  /// Dispose of the active exec cell. Exploring cells are absorbed into the
  /// turn-level accumulator (not written to scrollback). Non-exploring cells
  /// are written to scrollback as before. Invariant: after this call,
  /// `active_exec_cell == None` regardless of cell type.
  pub(super) fn flush_active_exec_cell(&mut self, app_event_tx: &AppEventSender) {
    if let Some(cell) = self.active_exec_cell.take() {
      // Exploring cells → absorb into accumulator instead of scrollback.
      if let Some(exec) = cell.as_any().downcast_ref::<crate::exec_cell::ExecCell>() {
        if exec.is_exploring_cell() {
          self.exploring_accumulator.absorb(exec);
          return;
        }
      }
      app_event_tx.insert_boxed_history_cell(cell);
    }
  }

  /// Flush the exploring accumulator to scrollback as a single summary cell.
  pub(super) fn flush_exploring_accumulator(&mut self, app_event_tx: &AppEventSender) {
    if let Some(cell) = self.exploring_accumulator.take_summary_cell() {
      app_event_tx.insert_boxed_history_cell(Box::new(cell));
    }
  }

  pub(super) fn flush_all_active_cells(&mut self, app_event_tx: &AppEventSender) {
    self.flush_active_exec_cell(app_event_tx);
    self.flush_exploring_accumulator(app_event_tx);
    if let Some(cell) = self.active_todo.take() {
      app_event_tx.insert_boxed_history_cell(Box::new(cell));
    }
    if let Some(cell) = self.active_collab_summary.take() {
      app_event_tx.insert_boxed_history_cell(cell);
    }
    if let Some(cell) = self.active_agent_preview.take() {
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
    self.exploring_accumulator.clear();
  }

  pub(super) fn clear_exec_state(&mut self) {
    self.pending_exec_calls.clear();
  }

  pub(crate) fn active_cell_transcript_key(&self) -> Option<ActiveCellTranscriptKey> {
    let has_active = self.active_collab_summary.is_some()
      || self.active_agent_preview.is_some()
      || self.active_exec_cell.is_some()
      || self.active_todo.is_some()
      || !self.exploring_accumulator.is_empty();
    if !has_active {
      return None;
    }

    let agent_preview = self.active_agent_preview.as_ref();
    let exec_animation_tick = self
      .active_exec_cell
      .as_ref()
      .and_then(|cell| cell.as_any().downcast_ref::<crate::exec_cell::ExecCell>())
      .and_then(|cell| cell.exploring_animation_tick());
    let animation_tick = agent_preview
      .and_then(|cell| cell.transcript_animation_tick())
      .or(exec_animation_tick);

    Some(ActiveCellTranscriptKey {
      revision: self.active_cell_revision,
      is_stream_continuation: agent_preview.is_some_and(|cell| cell.is_stream_continuation()),
      animation_tick,
    })
  }

  pub(crate) fn active_cell_transcript_lines(&self, width: u16) -> Option<Vec<Line<'static>>> {
    let width = width.max(1);
    let mut lines = Vec::new();

    let active_exploring = self
      .active_exec_cell
      .as_ref()
      .and_then(|c| c.as_any().downcast_ref::<crate::exec_cell::ExecCell>())
      .filter(|c| c.is_exploring_cell());
    let has_exploring = active_exploring.is_some() || !self.exploring_accumulator.is_empty();

    // 1. Collab summary (always first)
    if let Some(cell) = self.active_collab_summary.as_ref() {
      lines.extend(cell.transcript_lines(width));
    }

    // 2. Exploring summary OR agent_preview + exec_cell
    //    Order matches as_renderable(): exploring puts summary before agent_preview;
    //    non-exploring puts agent_preview before exec_cell.
    if has_exploring {
      let mut counts = self.exploring_accumulator.counts();
      if let Some(exec) = active_exploring {
        for call in exec.iter_calls().filter(|c| c.output.is_some()) {
          counts.add(&call.tool_name);
        }
      }
      if !counts.is_empty() {
        if !lines.is_empty() {
          lines.push(Line::from(""));
        }
        lines.push(counts.to_summary_line());
      } else if let Some(cell) = self.active_exec_cell.as_ref() {
        // No completed counts yet (first exploring call still running):
        // render the exploring cell directly (spinner).
        let exec_lines = cell.transcript_lines(width);
        if !exec_lines.is_empty() && !lines.is_empty() {
          lines.push(Line::from(""));
        }
        lines.extend(exec_lines);
      }
      // Agent preview after exploring summary
      if let Some(cell) = self.active_agent_preview.as_ref() {
        if !lines.is_empty() {
          lines.push(Line::from(""));
        }
        lines.extend(cell.transcript_lines(width));
      }
    } else {
      // Non-exploring: agent_preview first, then exec_cell
      if let Some(cell) = self.active_agent_preview.as_ref() {
        if !lines.is_empty() {
          lines.push(Line::from(""));
        }
        lines.extend(cell.transcript_lines(width));
      }
      if let Some(cell) = self.active_exec_cell.as_ref() {
        let exec_lines = cell.transcript_lines(width);
        if !exec_lines.is_empty() && !lines.is_empty() {
          lines.push(Line::from(""));
        }
        lines.extend(exec_lines);
      }
    }

    // 3. Todo (always last before bottom_pane)
    if let Some(cell) = self.active_todo.as_ref() {
      if !lines.is_empty() {
        lines.push(Line::from(""));
      }
      lines.extend(cell.display_lines(width));
    }

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
  use crate::app_event::AppEvent;
  use crate::app_event_sender::AppEventSender;
  use crate::history_cell::AgentMessageCell;
  use crate::history_cell::PlainHistoryCell;
  use ratatui::text::Line;
  use tokio::sync::mpsc::unbounded_channel;

  #[test]
  fn active_cell_transcript_cache_key_tracks_revision() {
    let mut state = ActiveTranscriptState::new(true);
    state.active_exec_cell = Some(Box::new(PlainHistoryCell::new(vec![Line::from("hello")])));

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
    state.active_exec_cell = Some(Box::new(PlainHistoryCell::new(vec![Line::from("tail")])));

    let lines = state.compose_alt_screen_lines(&[Line::from("history")], &[Line::from("tail")]);
    assert_eq!(
      lines,
      vec![Line::from("history"), Line::from(""), Line::from("tail")]
    );
  }

  #[test]
  fn active_cell_transcript_lines_include_agent_preview_and_exec_cell() {
    let mut state = ActiveTranscriptState::new(true);
    state.active_agent_preview = Some(Box::new(AgentMessageCell::new(
      vec![Line::from("preview")],
      true,
    )));
    state.active_exec_cell = Some(Box::new(PlainHistoryCell::new(vec![Line::from("exec")])));

    let rendered = state
      .active_cell_transcript_lines(80)
      .expect("combined active transcript")
      .iter()
      .map(Line::to_string)
      .collect::<Vec<_>>();
    assert_eq!(
      rendered,
      vec!["● preview".to_string(), "".to_string(), "exec".to_string()]
    );
  }

  #[test]
  fn flush_all_active_cells_flushes_exec_before_agent_preview() {
    let (tx, mut rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut state = ActiveTranscriptState::new(true);
    state.active_exec_cell = Some(Box::new(PlainHistoryCell::new(vec![Line::from("exec")])));
    state.active_agent_preview = Some(Box::new(AgentMessageCell::new(
      vec![Line::from("preview")],
      true,
    )));

    state.flush_all_active_cells(&sender);

    let Some(AppEvent::InsertHistoryCell(exec_cell)) = rx.try_recv().ok() else {
      panic!("expected exec cell to flush first");
    };
    let exec_rendered = exec_cell
      .display_lines(80)
      .iter()
      .map(Line::to_string)
      .collect::<Vec<_>>();
    assert_eq!(exec_rendered, vec!["exec".to_string()]);

    let Some(AppEvent::InsertHistoryCell(agent_cell)) = rx.try_recv().ok() else {
      panic!("expected agent preview to flush second");
    };
    let agent_rendered = agent_cell
      .display_lines(80)
      .iter()
      .map(Line::to_string)
      .collect::<Vec<_>>();
    assert_eq!(agent_rendered, vec!["● preview".to_string()]);
  }

  #[test]
  fn flush_all_active_cells_preserves_collab_summary_in_history() {
    let (tx, mut rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut state = ActiveTranscriptState::new(true);
    state.active_collab_summary = Some(Box::new(AgentMessageCell::new(
      vec![
        Line::from("Agent teams working..."),
        Line::from(" └─ @alpha"),
      ],
      true,
    )));

    state.flush_all_active_cells(&sender);

    let Some(AppEvent::InsertHistoryCell(cell)) = rx.try_recv().ok() else {
      panic!("expected collab summary to flush into history");
    };
    let rendered = cell
      .display_lines(80)
      .iter()
      .map(Line::to_string)
      .collect::<Vec<_>>()
      .join("\n");
    assert!(rendered.contains("Agent teams working..."));
    assert!(rendered.contains("@alpha"));
  }

  #[test]
  fn exploring_accumulator_produces_summary_in_transcript_lines() {
    let mut state = ActiveTranscriptState::new(true);
    // Simulate accumulator having absorbed some exploring calls.
    state.exploring_accumulator.calls.push(("read_file".to_string(), "foo.rs".to_string()));
    state.exploring_accumulator.calls.push(("read_file".to_string(), "bar.rs".to_string()));
    state.exploring_accumulator.calls.push(("search_tool".to_string(), "pattern".to_string()));

    let lines = state
      .active_cell_transcript_lines(80)
      .expect("should produce summary line");
    let rendered = lines.iter().map(Line::to_string).collect::<Vec<_>>().join("\n");
    assert!(rendered.contains("Explored"), "expected Explored label: {rendered}");
    assert!(rendered.contains("read 2 files"), "expected read count: {rendered}");
    assert!(rendered.contains("searched for 1 pattern"), "expected search count: {rendered}");
  }

  #[test]
  fn flush_all_active_cells_emits_exploring_summary_cell() {
    let (tx, mut rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut state = ActiveTranscriptState::new(true);
    state.exploring_accumulator.calls.push(("list_dir".to_string(), "src".to_string()));
    state.exploring_accumulator.calls.push(("list_dir".to_string(), "test".to_string()));

    state.flush_all_active_cells(&sender);

    let Some(AppEvent::InsertHistoryCell(cell)) = rx.try_recv().ok() else {
      panic!("expected exploring summary to flush into history");
    };
    let rendered = cell
      .display_lines(80)
      .iter()
      .map(Line::to_string)
      .collect::<Vec<_>>()
      .join("\n");
    assert!(rendered.contains("Explored"), "expected Explored: {rendered}");
    assert!(rendered.contains("listed 2 dirs"), "expected dir count: {rendered}");
    // Accumulator should be empty after flush.
    assert!(state.exploring_accumulator.is_empty());
  }
}
