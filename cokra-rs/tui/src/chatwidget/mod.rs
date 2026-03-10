mod exec_events;
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
use crate::history_cell::ApprovalRequestedHistoryCell;
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
  last_render_width: Cell<u16>,
  stream_render_mode: StreamRenderMode,
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
      bottom_pane: BottomPane::new(app_event_tx.clone(), frame_requester, animations_enabled),
      session: SessionState::default(),
      app_event_tx,
      last_render_width: Cell::new(0),
      stream_render_mode,
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
    self.transcript.flush_active_cell(&self.app_event_tx);
  }

  pub(crate) fn add_to_history(&mut self, cell: impl HistoryCell + 'static) {
    self.add_boxed_history(Box::new(cell));
  }

  fn add_boxed_history(&mut self, cell: Box<dyn HistoryCell>) {
    self.flush_active_cell();
    self.app_event_tx.insert_boxed_history_cell(cell);
  }

  fn append_boxed_history(&mut self, cell: Box<dyn HistoryCell>) {
    self.app_event_tx.insert_boxed_history_cell(cell);
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
  }

  fn bump_active_cell_revision(&mut self) {
    self.transcript.bump_active_cell_revision();
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
      .active_cell
      .as_mut()
      .and_then(|cell| cell.as_any_mut().downcast_mut::<AgentMessageCell>())
    {
      cell.replace_lines(lines);
    } else {
      self.flush_active_cell();
      self.transcript.active_cell = Some(Box::new(AgentMessageCell::new(lines, true)));
    }

    self.bump_active_cell_revision();
  }

  fn clear_streaming_agent_preview(&mut self) {
    let should_clear = self
      .transcript
      .active_cell
      .as_ref()
      .and_then(|cell| cell.as_any().downcast_ref::<AgentMessageCell>())
      .is_some();
    if should_clear {
      self.transcript.active_cell = None;
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
    let active_exec_call = self
      .transcript
      .active_cell
      .as_ref()
      .and_then(|cell| cell.as_any().downcast_ref::<crate::exec_cell::ExecCell>())
      .and_then(crate::exec_cell::ExecCell::active_call);

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
      EventMsg::AgentMessage(e) => self.on_agent_message(&e.item_id, &e.content),
      EventMsg::TokenCount(e) => self.on_token_count(e),
      EventMsg::SessionConfigured(e) => self.on_session_configured(e),
      EventMsg::ThreadNameUpdated(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "● Thread renamed: {}",
          e.name
        ))]));
      }
      EventMsg::ExecCommandBegin(e) => self.on_exec_command_begin(e),
      EventMsg::ExecCommandOutputDelta(e) => self.on_exec_command_output_delta(e),
      EventMsg::ExecCommandEnd(e) => self.on_exec_command_end(e),
      EventMsg::ExecApprovalRequest(e) => {
        self.add_to_history(ApprovalRequestedHistoryCell {
          command: e.command.clone(),
        });
        return Some(ChatWidgetAction::ShowApproval(e.clone()));
      }
      EventMsg::RequestUserInput(e) => {
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
    let active_cell_renderable = match &self.transcript.active_cell {
      Some(cell) => {
        RenderableItem::Borrowed(cell as &dyn Renderable).inset(Insets::tlbr(1, 0, 0, 0))
      }
      None => RenderableItem::Owned(Box::new(())),
    };
    let mut flex = FlexRenderable::new();
    flex.push(1, active_cell_renderable);
    flex.push(
      0,
      RenderableItem::Borrowed(&self.bottom_pane as &dyn Renderable)
        .inset(Insets::tlbr(1, 0, 0, 0)),
    );
    RenderableItem::Owned(Box::new(flex))
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
