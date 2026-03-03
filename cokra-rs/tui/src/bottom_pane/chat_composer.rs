use std::collections::HashMap;
use std::path::PathBuf;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

use super::textarea::TextArea;

#[derive(Debug, Clone)]
pub(crate) struct ComposerSubmission {
  pub(crate) text: String,
  pub(crate) local_image_attachments: HashMap<String, PathBuf>,
  pub(crate) remote_image_urls: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposerAction {
  None,
  Submit,
  Queue,
  Interrupt,
  RequestQuit,
}

#[derive(Debug)]
pub(crate) struct ChatComposer {
  textarea: TextArea,
  is_task_running: bool,
  local_image_attachments: HashMap<String, PathBuf>,
  remote_image_urls: Vec<String>,
}

impl ChatComposer {
  pub(crate) fn new() -> Self {
    Self {
      textarea: TextArea::new(),
      is_task_running: false,
      local_image_attachments: HashMap::new(),
      remote_image_urls: Vec::new(),
    }
  }

  pub(crate) fn set_task_running(&mut self, running: bool) {
    self.is_task_running = running;
  }

  pub(crate) fn handle_key_event(&mut self, key: KeyEvent) -> ComposerAction {
    self.handle_key_event_without_popup(key)
  }

  fn handle_key_event_without_popup(&mut self, key: KeyEvent) -> ComposerAction {
    match (key.code, key.modifiers) {
      (KeyCode::Enter, KeyModifiers::NONE) => {
        if self.is_task_running {
          ComposerAction::Queue
        } else {
          ComposerAction::Submit
        }
      }
      (KeyCode::Enter, _) => {
        self.textarea.insert_newline();
        ComposerAction::None
      }
      (KeyCode::Char('c'), mods)
        if mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::SHIFT) =>
      {
        if self.textarea.text_content().is_empty() {
          ComposerAction::Interrupt
        } else {
          ComposerAction::None
        }
      }
      (KeyCode::Char('u'), mods) if mods.contains(KeyModifiers::CONTROL) => {
        self.textarea.clear();
        ComposerAction::None
      }
      (KeyCode::Esc, _) => ComposerAction::RequestQuit,
      (KeyCode::Backspace, _) => {
        self.textarea.delete_char_backward();
        ComposerAction::None
      }
      (KeyCode::Left, _) => {
        self.textarea.move_cursor_left();
        ComposerAction::None
      }
      (KeyCode::Right, _) => {
        self.textarea.move_cursor_right();
        ComposerAction::None
      }
      (KeyCode::Up, _) => {
        self.textarea.move_cursor_up();
        ComposerAction::None
      }
      (KeyCode::Down, _) => {
        self.textarea.move_cursor_down();
        ComposerAction::None
      }
      (KeyCode::Home, _) => {
        self.textarea.move_cursor_home();
        ComposerAction::None
      }
      (KeyCode::End, _) => {
        self.textarea.move_cursor_end();
        ComposerAction::None
      }
      (KeyCode::Tab, KeyModifiers::NONE) => {
        if self.is_task_running {
          ComposerAction::Queue
        } else {
          self.textarea.insert_str("  ");
          ComposerAction::None
        }
      }
      (KeyCode::Char(ch), _) => {
        self.textarea.insert_char(ch);
        ComposerAction::None
      }
      _ => ComposerAction::None,
    }
  }

  pub(crate) fn handle_paste(&mut self, text: String) {
    self.textarea.insert_str(&text);
  }

  pub(crate) fn render_lines(
    &self,
    width: u16,
    show_cursor: bool,
  ) -> Vec<ratatui::text::Line<'static>> {
    self.textarea.render_lines(width, show_cursor)
  }

  pub(crate) fn cursor_render_position(&self, width: u16) -> (u16, u16) {
    self.textarea.cursor_render_position(width)
  }

  pub(crate) fn prepare_submission(&mut self) -> Option<ComposerSubmission> {
    let text = self.textarea.text_content();
    if text.trim().is_empty() && self.remote_image_urls.is_empty() {
      return None;
    }

    self.textarea.clear();
    Some(ComposerSubmission {
      text,
      local_image_attachments: self.local_image_attachments.clone(),
      remote_image_urls: self.remote_image_urls.clone(),
    })
  }
}
