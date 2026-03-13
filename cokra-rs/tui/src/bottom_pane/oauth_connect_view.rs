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
use ratatui::widgets::Wrap;

use super::bottom_pane_view::BottomPaneView;
use super::selection_popup_common::render_menu_surface;
use super::selection_popup_common::wrap_styled_line;
use super::textarea::TextArea;
use super::textarea::TextAreaState;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::render::renderable::Renderable;
use crate::tui::InlineViewportSizing;

pub(crate) struct OAuthConnectView {
  provider_id: String,
  provider_name: String,
  auth_url: String,
  instructions: String,
  prompt: String,
  auto_callback_enabled: bool,
  complete: bool,
  error_message: Option<String>,
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
    auto_callback_enabled: bool,
    app_event_tx: AppEventSender,
  ) -> Self {
    Self {
      provider_id,
      provider_name,
      auth_url,
      instructions,
      prompt,
      auto_callback_enabled,
      complete: false,
      error_message: None,
      textarea: TextArea::new(),
      textarea_state: RefCell::new(TextAreaState::default()),
      app_event_tx,
    }
  }

  fn submit(&mut self) {
    let input = self.textarea.text().trim().to_string();
    if input.is_empty() {
      self.error_message =
        Some("Paste the authorization code/URL, or press Esc to cancel.".to_string());
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
  fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
    self
  }

  fn inline_viewport_sizing(&self) -> InlineViewportSizing {
    // Tradeoff: keep OAuth login as a stable dialog in inline mode instead of
    // expanding the viewport, because resize redraws would otherwise push the
    // dialog itself into scrollback and duplicate its content.
    InlineViewportSizing::PreserveVisibleHistory
  }

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
        self.error_message = None;
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
    let content_width = width.saturating_sub(4);
    let header = [
      Line::from(format!("Connect provider: {}", self.provider_name).bold()),
      Line::from(self.instructions.clone().dim()),
    ];
    let header_height = header
      .iter()
      .map(|line| wrap_styled_line(line, content_width).len() as u16)
      .sum::<u16>();

    let mut info = vec![
      Line::from("Open this URL in your browser:".dim()),
      Line::from(self.auth_url.clone().cyan()),
      Line::from(self.prompt.clone().dim()),
    ];
    if self.auto_callback_enabled {
      info.push(Line::from(
        "Waiting for localhost callback; you can paste manually below.".dim(),
      ));
    }
    if let Some(error_message) = &self.error_message {
      info.push(Line::from(error_message.clone().red()));
    }

    let info_height = info
      .iter()
      .map(|line| wrap_styled_line(line, content_width).len() as u16)
      .sum::<u16>();

    let footer = Line::from("Enter to submit | Esc to cancel".dim());
    let footer_height = wrap_styled_line(&footer, content_width).len() as u16;

    header_height + info_height + footer_height + 1 + 4
  }

  fn render(&self, area: Rect, buf: &mut Buffer) {
    if area.is_empty() {
      return;
    }

    let content_area = render_menu_surface(area, buf);
    let content_width = content_area.width.max(1);
    let header = [
      Line::from(format!("Connect provider: {}", self.provider_name).bold()),
      Line::from(self.instructions.clone().dim()),
    ];
    let header_lines = header
      .iter()
      .flat_map(|line| wrap_styled_line(line, content_width))
      .collect::<Vec<_>>();

    let mut info = vec![
      Line::from("Open this URL in your browser:".dim()),
      Line::from(self.auth_url.clone().cyan()),
      Line::from(self.prompt.clone().dim()),
    ];
    if self.auto_callback_enabled {
      info.push(Line::from(
        "Waiting for localhost callback; you can paste manually below.".dim(),
      ));
    }
    if let Some(error_message) = &self.error_message {
      info.push(Line::from(error_message.clone().red()));
    }
    let info_lines = info
      .iter()
      .flat_map(|line| wrap_styled_line(line, content_width))
      .collect::<Vec<_>>();

    let footer = Line::from("Enter to submit | Esc to cancel".dim());
    let footer_lines = wrap_styled_line(&footer, content_width);
    let [header_area, info_area, input_area, footer_area] = Layout::vertical([
      Constraint::Length(header_lines.len() as u16),
      Constraint::Length(info_lines.len() as u16),
      Constraint::Length(1),
      Constraint::Length(footer_lines.len() as u16),
    ])
    .areas(content_area);

    Paragraph::new(header_lines)
      .wrap(Wrap { trim: false })
      .render(header_area, buf);

    Paragraph::new(info_lines)
      .wrap(Wrap { trim: false })
      .render(info_area, buf);

    let mut state = self.textarea_state.borrow_mut();
    (&self.textarea).render_ref(input_area, buf, &mut state);

    Paragraph::new(footer_lines)
      .wrap(Wrap { trim: false })
      .render(footer_area, buf);
  }

  fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
    let _ = area;
    None
  }
}

#[cfg(test)]
mod tests {
  use tokio::sync::mpsc;

  use super::*;

  #[test]
  fn oauth_connect_view_uses_stable_inline_viewport_sizing() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let view = OAuthConnectView::new(
      "anthropic-oauth".to_string(),
      "Anthropic".to_string(),
      "https://example.com/oauth".to_string(),
      "Log in with your browser.".to_string(),
      "Paste the redirect URL:".to_string(),
      true,
      AppEventSender::new(tx),
    );

    assert_eq!(
      <OAuthConnectView as BottomPaneView>::inline_viewport_sizing(&view),
      InlineViewportSizing::PreserveVisibleHistory
    );
  }

  #[test]
  fn oauth_connect_view_expands_height_for_wrapped_content() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let view = OAuthConnectView::new(
      "openai-codex".to_string(),
      "ChatGPT Plus/Pro".to_string(),
      "https://auth.openai.com/oauth/authorize?response_type=code&client_id=app_EMoamEEZ73f0CkXaXp7hrann&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback".to_string(),
      "Complete login in your browser. Cokra will try to capture the localhost callback automatically; if that does not work, paste the authorization code or full redirect URL here.".to_string(),
      "Paste the authorization code or redirect URL:".to_string(),
      true,
      AppEventSender::new(tx),
    );

    assert!(view.desired_height(50) > view.desired_height(140));
  }
}
