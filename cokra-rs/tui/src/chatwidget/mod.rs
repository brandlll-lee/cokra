mod exec_events;
mod interrupts;
mod notice_events;
mod session;
mod session_events;
mod stream_events;
mod transcript;

use std::path::PathBuf;
use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use cokra_protocol::EventMsg;
use cokra_protocol::ExecApprovalRequestEvent;
use cokra_protocol::RequestUserInputEvent;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::BottomPane;
use crate::history_cell::AgentMessageCell;
use crate::history_cell::HistoryCell;
use crate::history_cell::PlainHistoryCell;
use crate::history_cell::UserHistoryCell;
use crate::render::Insets;
use crate::render::renderable::FlexRenderable;
use crate::render::renderable::Renderable;
use crate::render::renderable::RenderableExt as _;
use crate::render::renderable::RenderableItem;
use crate::tui::FrameRequester;
use crate::tui::InlineViewportSizing;

use self::interrupts::InterruptManager;
use self::session::SessionState;
use self::session::StatusSnapshot;
pub use self::session::TokenUsage;
pub(crate) use self::transcript::ActiveCellTranscriptKey;
use self::transcript::ActiveTranscriptState;

use std::cell::Cell;

#[derive(Debug)]
pub(crate) enum ChatWidgetAction {
  ShowApproval(ExecApprovalRequestEvent),
  ShowRequestUserInput(RequestUserInputEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamRenderMode {
  AnimatedPreview,
  ScrollbackFirst,
}

pub(crate) struct ChatWidget {
  transcript: ActiveTranscriptState,
  pub(crate) bottom_pane: BottomPane,
  session: SessionState,
  app_event_tx: AppEventSender,
  frame_requester: FrameRequester,
  last_render_width: Cell<u16>,
  stream_render_mode: StreamRenderMode,
  interrupts: InterruptManager,
}

impl ChatWidget {
  pub(crate) fn new(
    app_event_tx: AppEventSender,
    frame_requester: FrameRequester,
    animations_enabled: bool,
    stream_render_mode: StreamRenderMode,
  ) -> Self {
    Self {
      transcript: ActiveTranscriptState::new(animations_enabled),
      bottom_pane: BottomPane::new(
        app_event_tx.clone(),
        frame_requester.clone(),
        animations_enabled,
      ),
      frame_requester,
      session: SessionState::default(),
      app_event_tx,
      last_render_width: Cell::new(0),
      stream_render_mode,
      interrupts: InterruptManager::default(),
    }
  }

  pub(crate) fn token_usage(&self) -> TokenUsage {
    self.session.token_usage()
  }

  pub(crate) fn cwd(&self) -> Option<&PathBuf> {
    self.session.cwd()
  }

  pub(crate) fn model_name(&self) -> &str {
    self.session.model_name()
  }

  pub(crate) fn set_model_name(&mut self, model_name: String) {
    self.session.set_model_name(model_name);
  }

  pub(crate) fn animations_enabled(&self) -> bool {
    self.transcript.animations_enabled()
  }

  pub(crate) fn active_cell_transcript_key(&self) -> Option<ActiveCellTranscriptKey> {
    self.transcript.active_cell_transcript_key()
  }

  pub(crate) fn active_cell_transcript_lines(&self, width: u16) -> Option<Vec<Line<'static>>> {
    self.transcript.active_cell_transcript_lines(width)
  }

  pub(crate) fn inline_viewport_sizing(&self) -> InlineViewportSizing {
    self.bottom_pane.inline_viewport_sizing()
  }

  fn streaming_wrap_width(&self) -> Option<usize> {
    let width = self.last_render_width.get();
    if width == 0 {
      return None;
    }
    // Reserve the agent gutter ("● " / "  ") so markdown tables are clamped before display.
    Some(width.saturating_sub(2).max(1) as usize)
  }

  fn flush_active_cell(&mut self) {
    self.transcript.flush_all_active_cells(&self.app_event_tx);
  }

  fn flush_active_exec_cell(&mut self) {
    self.transcript.flush_active_exec_cell(&self.app_event_tx);
  }

  /// Like `flush_active_exec_cell`, but skips the flush when the active cell
  /// is an exploring cell that has been visible for less than
  /// `MIN_EXPLORING_VISIBLE_MS`. This ensures users see at least a brief
  /// "⠋ Exploring" animation before the cell is replaced by streamed text.
  fn flush_active_exec_cell_if_visible_long_enough(&mut self) {
    const MIN_EXPLORING_VISIBLE_MS: u128 = 300;
    if let Some(cell) = self
      .transcript
      .active_exec_cell
      .as_ref()
      .and_then(|c| c.as_any().downcast_ref::<crate::exec_cell::ExecCell>())
    {
      if let Some(visible_since) = cell.exploring_visible_since {
        if visible_since.elapsed().as_millis() < MIN_EXPLORING_VISIBLE_MS {
          return;
        }
      }
    }
    self.flush_active_exec_cell();
  }

  pub(crate) fn add_to_history(&mut self, cell: impl HistoryCell + 'static) {
    self.add_boxed_history(Box::new(cell));
  }

  fn add_to_history_preserving_exec(&mut self, cell: impl HistoryCell + 'static) {
    self.app_event_tx.insert_boxed_history_cell(Box::new(cell));
  }

  fn add_boxed_history(&mut self, cell: Box<dyn HistoryCell>) {
    self.flush_active_exec_cell();
    self.app_event_tx.insert_boxed_history_cell(cell);
  }

  fn append_boxed_history(&mut self, cell: Box<dyn HistoryCell>) {
    self.app_event_tx.insert_boxed_history_cell(cell);
  }

  fn is_streaming(&self) -> bool {
    self.transcript.stream_controller.is_some()
  }

  /// Flush the answer stream: finalize the active stream controller and emit
  /// its buffered content as committed history.
  fn flush_answer_stream(&mut self) {
    let wrap_width = self.streaming_wrap_width();

    if let Some(filter) = self.transcript.xml_tool_filter.as_mut() {
      let remaining = filter.flush();
      if !remaining.is_empty() {
        let controller = self
          .transcript
          .stream_controller
          .get_or_insert_with(|| crate::streaming::controller::StreamController::new(wrap_width));
        controller.set_width_if_uncommitted(wrap_width);
        let _ = controller.push(&remaining);
        self.refresh_streaming_agent_preview();
      }
    }
    self.transcript.xml_tool_filter = None;

    if self.stream_render_mode == StreamRenderMode::ScrollbackFirst
      && let Some(controller) = self.transcript.stream_controller.as_mut()
      && let Some(cell) = controller.drain_committed_now()
    {
      self.append_boxed_history(cell);
    }

    if let Some(mut controller) = self.transcript.stream_controller.take()
      && let Some(cell) = controller.finalize()
    {
      match self.stream_render_mode {
        StreamRenderMode::AnimatedPreview => self.app_event_tx.insert_boxed_history_cell(cell),
        StreamRenderMode::ScrollbackFirst => self.append_boxed_history(cell),
      }
    }

    if self.stream_render_mode == StreamRenderMode::ScrollbackFirst {
      self.clear_streaming_agent_preview();
    }

    self.sync_status_indicator();
  }

  /// Defer an event if a stream is active, or handle it immediately.
  ///
  /// Once anything is queued we keep queueing to preserve FIFO ordering
  /// (e.g. ExecEnd must not arrive before its ExecBegin).
  #[inline]
  fn defer_or_handle(
    &mut self,
    push: impl FnOnce(&mut InterruptManager),
    handle: impl FnOnce(&mut Self),
  ) {
    if self.is_streaming() || !self.interrupts.is_empty() {
      push(&mut self.interrupts);
    } else {
      handle(self);
    }
  }

  fn flush_interrupt_queue(&mut self) -> Option<ChatWidgetAction> {
    let mut mgr = std::mem::take(&mut self.interrupts);
    let action = mgr.flush_all(self);
    self.interrupts = mgr;
    action
  }

  fn flush_stream_controllers(&mut self) {
    let wrap_width = self.streaming_wrap_width();

    if let Some(filter) = self.transcript.xml_tool_filter.as_mut() {
      let remaining = filter.flush();
      if !remaining.is_empty() {
        let controller = self
          .transcript
          .stream_controller
          .get_or_insert_with(|| crate::streaming::controller::StreamController::new(wrap_width));
        controller.set_width_if_uncommitted(wrap_width);
        let _ = controller.push(&remaining);
        self.refresh_streaming_agent_preview();
      }
    }
    self.transcript.xml_tool_filter = None;

    if self.stream_render_mode == StreamRenderMode::ScrollbackFirst
      && let Some(controller) = self.transcript.stream_controller.as_mut()
      && let Some(cell) = controller.drain_committed_now()
    {
      self.append_boxed_history(cell);
    }

    let stream_controller = self.transcript.stream_controller.take();
    let plan_stream_controller = self.transcript.plan_stream_controller.take();

    if let Some(mut controller) = stream_controller
      && self.stream_render_mode == StreamRenderMode::ScrollbackFirst
      && let Some(cell) = controller.finalize()
    {
      self.append_boxed_history(cell);
    }

    if let Some(mut controller) = plan_stream_controller
      && let Some(cell) = controller.finalize()
    {
      match self.stream_render_mode {
        StreamRenderMode::AnimatedPreview => self.add_boxed_history(cell),
        StreamRenderMode::ScrollbackFirst => self.append_boxed_history(cell),
      }
    }

    if self.stream_render_mode == StreamRenderMode::ScrollbackFirst {
      self.clear_streaming_agent_preview();
    }

    // The stream is now finalized — flush any deferred interrupts.
    self.flush_interrupt_queue();
  }

  fn bump_active_cell_revision(&mut self) {
    self.transcript.bump_active_cell_revision();
    self.frame_requester.schedule_frame();
    // If there is a live exploring cell, schedule a follow-up frame after one
    // spinner interval so the spinner keeps animating even when no new events
    // arrive (e.g. between exec_end and the next exec_begin).
    if self.has_live_exploring_cell() {
      self
        .frame_requester
        .schedule_frame_in(std::time::Duration::from_millis(
          crate::exec_cell::SPINNER_INTERVAL_MS as u64,
        ));
    }
  }

  /// Returns true when there is an active exploring group in the viewport
  /// with at least one unfinished explore call. Used to drive continuous
  /// spinner animation frames only while the group is actually exploring.
  pub(crate) fn has_live_exploring_cell(&self) -> bool {
    self
      .transcript
      .active_exec_cell
      .as_ref()
      .and_then(|cell| cell.as_any().downcast_ref::<crate::exec_cell::ExecCell>())
      .is_some_and(|cell| cell.is_exploring_cell() && cell.is_active())
  }

  fn refresh_streaming_agent_preview(&mut self) {
    let Some(controller) = self.transcript.stream_controller.as_mut() else {
      self.clear_streaming_agent_preview();
      return;
    };
    let lines = match self.stream_render_mode {
      StreamRenderMode::AnimatedPreview => {
        // Tradeoff: keep alt-screen on the legacy full-buffer preview so we can
        // modernize inline terminal behavior without simultaneously changing the
        // separate alt-screen transcript experience.
        controller.discard_queued();
        controller.preview_lines()
      }
      StreamRenderMode::ScrollbackFirst => controller.preview_uncommitted_lines(),
    };
    if lines.is_empty() {
      self.clear_streaming_agent_preview();
      return;
    }

    if let Some(cell) = self
      .transcript
      .active_agent_preview
      .as_mut()
      .and_then(|cell| cell.as_any_mut().downcast_mut::<AgentMessageCell>())
    {
      cell.replace_lines(lines);
    } else {
      self.transcript.active_agent_preview = Some(Box::new(AgentMessageCell::new(lines, true)));
    }

    self.bump_active_cell_revision();
  }

  fn clear_streaming_agent_preview(&mut self) {
    let should_clear = self
      .transcript
      .active_agent_preview
      .as_ref()
      .and_then(|cell| cell.as_any().downcast_ref::<AgentMessageCell>())
      .is_some();
    if should_clear {
      self.transcript.active_agent_preview = None;
      self.bump_active_cell_revision();
    }
  }

  pub(crate) fn set_agent_turn_running(&mut self, running: bool) {
    self.session.agent_turn_running = running;
    self.bottom_pane.set_task_running(running);
    if running {
      self.sync_status_indicator();
    }
  }

  fn current_exec_status_snapshot(&self) -> Option<StatusSnapshot> {
    let active_exec_cell = self
      .transcript
      .active_exec_cell
      .as_ref()
      .and_then(|cell| cell.as_any().downcast_ref::<crate::exec_cell::ExecCell>());

    // Exploring groups own their own transcript-local status. Keep the bottom
    // pane on the global Working surface instead of swapping it to a per-tool
    // "Running ..." label for read/list/search activity.
    if active_exec_cell.is_some_and(crate::exec_cell::ExecCell::is_exploring_cell) {
      return None;
    }

    let active_exec_call = active_exec_cell.and_then(crate::exec_cell::ExecCell::active_call);

    active_exec_call.map(|call| {
      StatusSnapshot::new(
        format!("Running {}", call.tool_name),
        Some(call.command.clone()),
        None,
      )
    })
  }

  fn current_mcp_status_snapshot(&self) -> Option<StatusSnapshot> {
    if self.session.mcp_starting_servers.is_empty() {
      return None;
    }

    let servers = self
      .session
      .mcp_starting_servers
      .iter()
      .cloned()
      .collect::<Vec<_>>();
    let header = match servers.as_slice() {
      [server] => format!("Booting MCP server: {server}"),
      [first, second] => format!("Starting MCP servers: {first}, {second}"),
      [first, second, third, ..] => {
        format!("Starting MCP servers: {first}, {second}, {third}, …")
      }
      [] => return None,
    };
    let details = (servers.len() > 1).then(|| servers.join("\n"));

    Some(StatusSnapshot::new(header, details, None))
  }

  fn derived_status_snapshot(&self) -> StatusSnapshot {
    self
      .current_exec_status_snapshot()
      .or_else(|| self.session.collab_wait_status.clone())
      .or_else(|| self.session.active_status_override.clone())
      .or_else(|| self.current_mcp_status_snapshot())
      .or_else(|| {
        extract_first_bold(&self.session.reasoning_buffer)
          .map(|header| StatusSnapshot::new(header, None, None))
      })
      .unwrap_or_else(StatusSnapshot::working)
  }

  fn sync_status_indicator(&mut self) {
    let snapshot = self.derived_status_snapshot();
    let Some(status) = self.bottom_pane.status_widget_mut() else {
      return;
    };
    status.update_header(snapshot.header);
    status.update_details(snapshot.details);
    status.update_inline_message(snapshot.inline_message);
  }

  fn set_status_override(
    &mut self,
    header: impl Into<String>,
    details: Option<String>,
    inline_message: Option<String>,
  ) {
    self.session.active_status_override =
      Some(StatusSnapshot::new(header, details, inline_message));
    self.sync_status_indicator();
  }

  fn clear_status_override(&mut self) {
    self.session.active_status_override = None;
    self.sync_status_indicator();
  }

  fn set_collab_wait_status(&mut self, status: Option<StatusSnapshot>) {
    self.session.collab_wait_status = status;
    self.sync_status_indicator();
  }

  fn on_reasoning_delta(&mut self, delta: &str) {
    self.session.reasoning_buffer.push_str(delta);
    self.sync_status_indicator();
  }

  fn on_reasoning_final(&mut self, text: &str) {
    self.session.reasoning_buffer.clear();
    self.session.reasoning_buffer.push_str(text);
    self.sync_status_indicator();
  }

  fn on_reasoning_section_break(&mut self) {
    self.session.reasoning_buffer.clear();
    self.sync_status_indicator();
  }

  fn run_commit_tick_with_scope(&mut self, scope: crate::streaming::commit_tick::CommitTickScope) {
    let output = self.transcript.on_commit_tick(scope, Instant::now());

    for cell in output.cells {
      self.add_boxed_history(cell);
    }

    if output.has_controller && output.all_idle {
      self.app_event_tx.send(AppEvent::StopCommitAnimation);
    }

    if output.all_idle
      && !self.session.agent_turn_running
      && let Some(status) = self.bottom_pane.status_widget_mut()
    {
      status.pause_timer();
    }
  }

  fn run_catch_up_commit_tick(&mut self) {
    self.run_commit_tick_with_scope(crate::streaming::commit_tick::CommitTickScope::CatchUpOnly);
  }

  pub(crate) fn open_resume_picker(&mut self) {
    self.add_to_history(PlainHistoryCell::new(vec![Line::from(
      "● /resume is not yet implemented.".dim(),
    )]));
  }

  pub(crate) fn handle_event(&mut self, event: &EventMsg) -> Option<ChatWidgetAction> {
    if self.handle_notice_event(event) {
      return None;
    }

    match event {
      EventMsg::UserMessage(e) => {
        let mut text_parts: Vec<String> = Vec::new();
        let mut text_elements = Vec::new();
        let mut remote_image_urls = Vec::new();
        let mut byte_offset = 0usize;

        for item in &e.items {
          match item {
            cokra_protocol::UserInput::Text {
              text,
              text_elements: elems,
            } => {
              if !text_parts.is_empty() {
                byte_offset += 1;
              }
              for elem in elems {
                text_elements.push(cokra_protocol::TextElement {
                  byte_range: cokra_protocol::ByteRange {
                    start: elem.byte_range.start + byte_offset,
                    end: elem.byte_range.end + byte_offset,
                  },
                  placeholder: elem.placeholder.clone(),
                });
              }
              byte_offset += text.len();
              text_parts.push(text.clone());
            }
            cokra_protocol::UserInput::Image { image_url } => {
              remote_image_urls.push(image_url.clone());
            }
            cokra_protocol::UserInput::LocalImage { path } => {
              text_parts.push(format!("[local_image] {}", path.display()));
              byte_offset = text_parts.iter().map(|s| s.len()).sum::<usize>()
                + text_parts.len().saturating_sub(1);
            }
            cokra_protocol::UserInput::Skill { name, .. } => {
              text_parts.push(format!("[skill] {name}"));
              byte_offset = text_parts.iter().map(|s| s.len()).sum::<usize>()
                + text_parts.len().saturating_sub(1);
            }
            cokra_protocol::UserInput::Mention { name, path } => {
              text_parts.push(format!("[@{name}] {path}"));
              byte_offset = text_parts.iter().map(|s| s.len()).sum::<usize>()
                + text_parts.len().saturating_sub(1);
            }
          }
        }

        let text = text_parts.join("\n");
        self.add_to_history(UserHistoryCell::new(text, text_elements, remote_image_urls));
      }
      EventMsg::TurnStarted(e) => self.on_turn_started(e),
      EventMsg::AgentMessageDelta(e) | EventMsg::AgentMessageContentDelta(e) => {
        self.on_agent_message_delta(&e.item_id, &e.delta);
      }
      EventMsg::AgentMessage(e) => {
        if let Some(action) = self.on_agent_message(&e.item_id, &e.content) {
          return Some(action);
        }
      }
      EventMsg::TokenCount(e) => self.on_token_count(e),
      EventMsg::SessionConfigured(e) => self.on_session_configured(e),
      EventMsg::ThreadNameUpdated(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "● Thread renamed: {}",
          e.name
        ))]));
      }
      EventMsg::ExecCommandBegin(e) => {
        // 1:1 Codex: flush the answer stream before deferring so the exec
        // begin is handled immediately instead of being queued behind the
        // active text stream.  This makes the Explored list grow in real-time.
        self.flush_answer_stream();
        let e2 = e.clone();
        self.defer_or_handle(
          |q| q.push_exec_begin(e.clone()),
          |s| s.handle_exec_begin_now(&e2),
        );
      }
      EventMsg::ExecCommandOutputDelta(e) => self.on_exec_command_output_delta(e),
      EventMsg::ExecCommandEnd(e) => {
        // 1:1 Codex: flush the answer stream before deferring so the exec
        // end is handled immediately.
        self.flush_answer_stream();
        let e2 = e.clone();
        self.defer_or_handle(
          |q| q.push_exec_end(e.clone()),
          |s| s.handle_exec_end_now(&e2),
        );
      }
      EventMsg::ExecApprovalRequest(e) => {
        // Approval requests must NEVER be deferred: the agent halts and waits
        // for the user's decision, so no further stream deltas will arrive.
        // Deferring would cause a deadlock (stream waits for approval, approval
        // waits for stream-end). Flush the stream + interrupt queue first, then
        // show the approval overlay immediately.
        self.flush_stream_controllers();
        return Some(self.handle_exec_approval_now(e.clone()));
      }
      EventMsg::RequestUserInput(e) => {
        // Same reasoning as ExecApprovalRequest: agent blocks waiting for the
        // user, so deferring would deadlock. Flush stream then show immediately.
        self.flush_stream_controllers();
        return Some(ChatWidgetAction::ShowRequestUserInput(e.clone()));
      }
      EventMsg::Warning(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
          Span::from("warning: ").yellow(),
          Span::from(e.message.clone()),
        ])]));
      }
      EventMsg::StreamError(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
          Span::from("stream error: ").red(),
          Span::from(e.error.clone()),
        ])]));
      }
      EventMsg::Error(e) => self.on_error(e),
      EventMsg::TurnComplete(e) => self.on_turn_complete(e),
      EventMsg::TurnAborted(e) => self.on_turn_aborted(e),
      EventMsg::RawResponseItem(_) => {}
      EventMsg::PlanDelta(e) => self.on_plan_delta(&e.delta),
      EventMsg::ReasoningContentDelta(e) => self.on_reasoning_delta(&e.delta),
      EventMsg::ReasoningRawContentDelta(e) => self.on_reasoning_delta(&e.delta),
      _ => {}
    }

    None
  }

  fn as_renderable(&self) -> RenderableItem<'_> {
    let mut live_content = FlexRenderable::new();
    let has_exploring_exec = self
      .transcript
      .active_exec_cell
      .as_ref()
      .and_then(|cell| cell.as_any().downcast_ref::<crate::exec_cell::ExecCell>())
      .is_some_and(crate::exec_cell::ExecCell::is_exploring_cell);
    if let Some(cell) = &self.transcript.active_agent_preview {
      live_content.push(
        if has_exploring_exec { 1 } else { 0 },
        RenderableItem::Borrowed(cell as &dyn Renderable).inset(Insets::tlbr(1, 0, 0, 0)),
      );
    }
    if let Some(cell) = &self.transcript.active_exec_cell {
      live_content.push(
        if has_exploring_exec { 0 } else { 1 },
        RenderableItem::Borrowed(cell as &dyn Renderable).inset(Insets::tlbr(1, 0, 0, 0)),
      );
    }

    let mut outer = FlexRenderable::new();
    outer.push(1, RenderableItem::Owned(Box::new(live_content)));
    outer.push(
      0,
      RenderableItem::Borrowed(&self.bottom_pane as &dyn Renderable)
        .inset(Insets::tlbr(1, 0, 0, 0)),
    );
    RenderableItem::Owned(Box::new(outer))
  }

  pub(crate) fn render_alt_screen(
    &self,
    area: Rect,
    buf: &mut Buffer,
    alt_history_lines: &[Line<'static>],
    active_tail_lines: &[Line<'static>],
    scroll_offset: u16,
  ) {
    if area.height == 0 || area.width == 0 {
      return;
    }

    let lines = self
      .transcript
      .compose_alt_screen_lines(alt_history_lines, active_tail_lines);

    let overflow = lines.len().saturating_sub(usize::from(area.height));
    let scroll_y = if scroll_offset == 0 {
      u16::try_from(overflow).unwrap_or(u16::MAX)
    } else {
      let bounded = overflow.saturating_sub(scroll_offset as usize);
      u16::try_from(bounded).unwrap_or(u16::MAX)
    };

    Paragraph::new(Text::from(lines))
      .scroll((scroll_y, 0))
      .render(area, buf);
  }
}

// Extract the first Markdown bold element in the form `**...**`.
// Tradeoff: we intentionally stop at the first unmatched opener so streaming
// partial chunks don't produce flickering half-headers.
fn extract_first_bold(s: &str) -> Option<String> {
  let bytes = s.as_bytes();
  let mut i = 0usize;
  while i + 1 < bytes.len() {
    if bytes[i] == b'*' && bytes[i + 1] == b'*' {
      let start = i + 2;
      let mut j = start;
      while j + 1 < bytes.len() {
        if bytes[j] == b'*' && bytes[j + 1] == b'*' {
          let inner = &s[start..j];
          let trimmed = inner.trim();
          return (!trimmed.is_empty()).then(|| trimmed.to_string());
        }
        j += 1;
      }
      return None;
    }
    i += 1;
  }
  None
}

impl Renderable for ChatWidget {
  fn render(&self, area: Rect, buf: &mut Buffer) {
    self.last_render_width.set(area.width);
    self.as_renderable().render(area, buf);
  }

  fn desired_height(&self, width: u16) -> u16 {
    self.as_renderable().desired_height(width)
  }

  fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
    self.as_renderable().cursor_pos(area)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::app_event_sender::AppEventSender;
  use crate::exec_cell::new_active_exec_command;
  use crate::history_cell::AgentMessageCell;
  use ratatui::Terminal;
  use ratatui::backend::TestBackend;
  use tokio::sync::mpsc::unbounded_channel;

  fn render_chat_widget(widget: &ChatWidget, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
      .draw(|f| widget.render(f.area(), f.buffer_mut()))
      .expect("draw");
    format!("{}", terminal.backend())
  }

  #[test]
  fn inline_render_keeps_exploring_header_visible_with_agent_preview() {
    let (tx, _rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut widget = ChatWidget::new(
      sender,
      FrameRequester::test_dummy(),
      false,
      StreamRenderMode::ScrollbackFirst,
    );
    widget.set_agent_turn_running(true);
    widget.transcript.active_agent_preview = Some(Box::new(AgentMessageCell::new(
      vec![
        Line::from("我已经定位到核心实现都在 core/src/agent 和 core/src/tools/handlers。"),
        Line::from("接着读这些关键文件，整理输出功能边界和典型工作流。"),
      ],
      true,
    )));
    widget.transcript.active_exec_cell = Some(Box::new(new_active_exec_command(
      "call-1".to_string(),
      "search_tool".to_string(),
      "handle_mcp_command list_tools new_streamable_http_client new_stdio_client McpServerTransportConfig required enabled tool_timeout include_tools exclude_tools".to_string(),
      std::path::PathBuf::from("/tmp/project"),
      false,
    )));

    let rendered = render_chat_widget(&widget, 80, 10);
    assert!(
      rendered.contains("Exploring"),
      "expected exploring header to remain visible: {rendered}"
    );
    assert!(
      rendered.contains("Search handle_mcp_command"),
      "expected latest explore summary to remain visible: {rendered}"
    );
    assert!(
      rendered.contains("Working"),
      "expected bottom working row to remain visible: {rendered}"
    );
    assert!(
      rendered.contains("Type @ to mention files"),
      "expected composer placeholder to remain visible: {rendered}"
    );
  }
}
