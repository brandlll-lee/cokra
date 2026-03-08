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
use ratatui::widgets::StatefulWidgetRef;
use ratatui::widgets::Widget;

use super::bottom_pane_view::BottomPaneView;
use super::selection_popup_common::render_menu_surface;
use super::selection_popup_common::wrap_styled_line;
use super::textarea::TextArea;
use super::textarea::TextAreaState;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::render::renderable::Renderable;

pub(crate) struct OAuthConnectView {
  provider_id: String,
  provider_name: String,
  auth_url: String,
  instructions: String,
  prompt: String,
  complete: bool,
  textarea: TextArea,
  textarea_state: RefCell<TextAreaState>,
  app_event_tx: AppEventSender,
}

impl OAuthConnectView {
  pub(crate) fn new(
    provider_id: String,
    provider_name: String,
    auth_url: String,
    instructions: String,
    prompt: String,
    app_event_tx: AppEventSender,
  ) -> Self {
    Self {
      provider_id,
      provider_name,
      auth_url,
      instructions,
      prompt,
      complete: false,
      textarea: TextArea::new(),
      textarea_state: RefCell::new(TextAreaState::default()),
      app_event_tx,
    }
  }

  fn submit(&mut self) {
    let input = self.textarea.text().trim().to_string();
    if input.is_empty() {
      self.complete = true;
      return;
    }
    self.app_event_tx.send(AppEvent::OAuthCodeSubmitted {
      provider_id: self.provider_id.clone(),
      input,
    });
    self.complete = true;
  }

  fn cancel(&mut self) {
    self.app_event_tx.send(AppEvent::CancelOAuthConnect {
      provider_id: self.provider_id.clone(),
    });
    self.complete = true;
  }
}

impl BottomPaneView for OAuthConnectView {
  fn handle_key_event(&mut self, key_event: KeyEvent) {
    match key_event {
      KeyEvent {
        code: KeyCode::Esc, ..
      } => {
        self.cancel();
      }
      KeyEvent {
        code: KeyCode::Char('c'),
        modifiers,
        ..
      } if modifiers.contains(KeyModifiers::CONTROL) => {
        self.cancel();
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
    self.cancel();
    true
  }
}

impl Renderable for OAuthConnectView {
  fn desired_height(&self, width: u16) -> u16 {
    let mut height = 0;
    let header = [
      Line::from(format!("Connect provider: {}", self.provider_name).bold()),
      Line::from(self.instructions.clone().dim()),
    ];
    height += header.len() as u16;

    for line in [
      Line::from("Open this URL in your browser:".dim()),
      Line::from(self.auth_url.clone().cyan()),
      Line::from(self.prompt.clone().dim()),
    ] {
      height += wrap_styled_line(&line, width.saturating_sub(2)).len() as u16;
    }

    let footer = Line::from("Enter to submit · Esc to cancel".dim());
    height += wrap_styled_line(&footer, width.saturating_sub(2)).len() as u16;

    height + 1 + 4
  }

  fn render(&self, area: Rect, buf: &mut Buffer) {
    if area.is_empty() {
      return;
    }

    let content_area = render_menu_surface(area, buf);
    let [header_area, info_area, input_area, footer_area] = Layout::vertical([
      Constraint::Length(2),
      Constraint::Min(3),
      Constraint::Length(1),
      Constraint::Min(1),
    ])
    .areas(content_area);

    Paragraph::new(vec![
      Line::from(format!("Connect provider: {}", self.provider_name).bold()),
      Line::from(self.instructions.clone().dim()),
    ])
    .render(header_area, buf);

    Paragraph::new(vec![
      Line::from("Open this URL in your browser:".dim()),
      Line::from(self.auth_url.clone().cyan()),
      Line::from(self.prompt.clone().dim()),
    ])
    .render(info_area, buf);

    let mut state = self.textarea_state.borrow_mut();
    (&self.textarea).render_ref(input_area, buf, &mut state);

    if let Some(line) = wrap_styled_line(
      &Line::from("Enter to submit · Esc to cancel".dim()),
      footer_area.width.saturating_sub(2),
    )
    .into_iter()
    .next()
    {
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
