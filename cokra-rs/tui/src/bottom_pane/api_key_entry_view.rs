use std::cell::RefCell;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use super::bottom_pane_view::BottomPaneView;
use super::selection_popup_common::render_menu_surface;
use super::selection_popup_common::wrap_styled_line;
use super::textarea::TextArea;
use super::textarea::TextAreaState;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::render::renderable::Renderable;

pub(crate) struct ApiKeyEntryView {
  provider_id: String,
  model_id: String,
  effort: Option<cokra_protocol::ReasoningEffortConfig>,
  complete: bool,
  textarea: TextArea,
  textarea_state: RefCell<TextAreaState>,
  app_event_tx: AppEventSender,
}

impl ApiKeyEntryView {
  pub(crate) fn new(
    provider_id: String,
    model_id: String,
    effort: Option<cokra_protocol::ReasoningEffortConfig>,
    app_event_tx: AppEventSender,
  ) -> Self {
    Self {
      provider_id,
      model_id,
      effort,
      complete: false,
      textarea: TextArea::new(),
      textarea_state: RefCell::new(TextAreaState::default()),
      app_event_tx,
    }
  }

  fn submit(&mut self) {
    let api_key = self.textarea.text().trim().to_string();
    if api_key.is_empty() {
      self.complete = true;
      return;
    }

    self.app_event_tx.send(AppEvent::ApiKeySubmitted {
      provider_id: self.provider_id.clone(),
      api_key,
      model_id: self.model_id.clone(),
      effort: self.effort.clone(),
    });
    self.complete = true;
  }
}

impl BottomPaneView for ApiKeyEntryView {
  fn handle_key_event(&mut self, key_event: KeyEvent) {
    match key_event {
      KeyEvent {
        code: KeyCode::Esc,
        ..
      } => {
        self.complete = true;
      }
      KeyEvent {
        code: KeyCode::Char('c'),
        modifiers,
        ..
      } if modifiers.contains(KeyModifiers::CONTROL) => {
        self.complete = true;
      }
      KeyEvent {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::NONE,
        ..
      } => {
        self.submit();
      }
      other => {
        self.textarea.input(other);
      }
    }
  }

  fn is_complete(&self) -> bool {
    self.complete
  }

  fn on_cancel(&mut self) -> bool {
    self.complete = true;
    true
  }
}

impl Renderable for ApiKeyEntryView {
  fn desired_height(&self, width: u16) -> u16 {
    let mut height = 0;
    let header = vec![
      Line::from(format!("Connect provider: {}", self.provider_id).bold()),
      Line::from("Enter API key / token (will not be displayed).".dim()),
    ];
    height += header.len() as u16;

    let footer = Line::from("Enter to submit · Esc to cancel".dim());
    let footer_lines = wrap_styled_line(&footer, width.saturating_sub(2));
    height += footer_lines.len() as u16;

    height + 1 + 1 + 4
  }

  fn render(&self, area: Rect, buf: &mut Buffer) {
    if area.is_empty() {
      return;
    }

    let content_area = render_menu_surface(area, buf);

    let [header_area, input_area, footer_area] = Layout::vertical([
      Constraint::Length(2),
      Constraint::Length(1),
      Constraint::Min(1),
    ])
    .areas(content_area);

    let header = vec![
      Line::from(format!("Connect provider: {}", self.provider_id).bold()),
      Line::from("Enter API key / token (will not be displayed).".dim()),
    ];
    Paragraph::new(header).render(header_area, buf);

    let mut state = self.textarea_state.borrow_mut();
    self
      .textarea
      .render_ref_masked(input_area, buf, &mut state, '•');

    let footer = Line::from("Enter to submit · Esc to cancel".dim());
    let wrapped = wrap_styled_line(&footer, footer_area.width.saturating_sub(2));
    if let Some(line) = wrapped.into_iter().next() {
      let footer_area = Rect {
        x: footer_area.x + 2,
        y: footer_area.y,
        width: footer_area.width.saturating_sub(2),
        height: 1,
      };
      line.render(footer_area, buf);
    }
  }

  fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
    let _ = area;
    None
  }
}
