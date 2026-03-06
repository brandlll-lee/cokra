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

use self::session::SessionState;
pub use self::session::TokenUsage;
pub(crate) use self::transcript::ActiveCellTranscriptKey;
use self::transcript::ActiveTranscriptState;

#[derive(Debug)]
pub(crate) enum ChatWidgetAction {
  ShowApproval(ExecApprovalRequestEvent),
  ShowRequestUserInput(RequestUserInputEvent),
}

pub(crate) struct ChatWidget {
  transcript: ActiveTranscriptState,
  pub(crate) bottom_pane: BottomPane,
  session: SessionState,
  app_event_tx: AppEventSender,
}

impl ChatWidget {
  pub(crate) fn new(
    app_event_tx: AppEventSender,
    frame_requester: FrameRequester,
    animations_enabled: bool,
  ) -> Self {
    Self {
      transcript: ActiveTranscriptState::new(animations_enabled),
      bottom_pane: BottomPane::new(app_event_tx.clone(), frame_requester, animations_enabled),
      session: SessionState::default(),
      app_event_tx,
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

  fn flush_stream_controllers(&mut self) {
    self.transcript.flush_stream_controllers(&self.app_event_tx);
  }

  fn bump_active_cell_revision(&mut self) {
    self.transcript.bump_active_cell_revision();
  }

  pub(crate) fn set_agent_turn_running(&mut self, running: bool) {
    self.session.agent_turn_running = running;
    self.bottom_pane.set_task_running(running);
  }

  pub(crate) fn push_user_input_text(&mut self, text: String) {
    self.add_to_history(UserHistoryCell::from_text(text));
  }

  pub(crate) fn open_resume_picker(&mut self) {
    self.add_to_history(PlainHistoryCell::new(vec![Line::from(
      "• /resume is not yet implemented.".dim(),
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
          "• Thread renamed: {}",
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
      EventMsg::TurnComplete(_) => self.on_turn_complete(),
      EventMsg::TurnAborted(e) => self.on_turn_aborted(e),
      EventMsg::RawResponseItem(_) => {}
      EventMsg::PlanDelta(e) => self.on_plan_delta(&e.delta),
      EventMsg::ReasoningContentDelta(_) | EventMsg::ReasoningRawContentDelta(_) => {}
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

impl Renderable for ChatWidget {
  fn render(&self, area: Rect, buf: &mut Buffer) {
    self.as_renderable().render(area, buf);
  }

  fn desired_height(&self, width: u16) -> u16 {
    self.as_renderable().desired_height(width)
  }

  fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
    self.as_renderable().cursor_pos(area)
  }
}
