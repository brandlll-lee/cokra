use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use cokra_protocol::user_input::TextElement;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::StatefulWidgetRef;
use ratatui::widgets::Widget;

use super::MentionBinding;
use super::chat_composer_history::ChatComposerHistory;
use super::chat_composer_history::HistoryEntry;
use super::command_popup::CommandPopup;
use super::command_popup::CommandPopupAction;
use super::footer;
use super::footer::CollaborationModeIndicator;
use super::footer::FooterMode;
use super::footer::FooterProps;
use super::paste_burst::CharDecision;
use super::paste_burst::FlushResult;
use super::paste_burst::PasteBurst;
use super::textarea::TextArea;
use super::textarea::TextAreaState;
use crate::key_hint;
use crate::key_hint::has_ctrl_or_alt;
use crate::render::renderable::Renderable;
use crate::slash_command::SlashCommand;

const FOOTER_TRANSIENT_HINT_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub(crate) struct ComposerSubmission {
  pub(crate) text: String,
  pub(crate) text_elements: Vec<TextElement>,
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
  textarea_state: RefCell<TextAreaState>,
  is_task_running: bool,
  footer_mode: FooterMode,
  footer_timer: Option<Instant>,
  paste_burst: PasteBurst,
  command_popup: Option<CommandPopup>,
  history: ChatComposerHistory,
  input_enabled: bool,
  steer_enabled: bool,
  collaboration_modes_enabled: bool,
  collaboration_mode_indicator: Option<CollaborationModeIndicator>,
  context_window_percent: Option<i64>,
  context_window_used_tokens: Option<i64>,
  status_line_value: Option<Line<'static>>,
  status_line_enabled: bool,
  local_image_attachments: HashMap<String, PathBuf>,
  remote_image_urls: Vec<String>,
  mention_bindings: Vec<MentionBinding>,
  pending_pastes: Vec<(String, String)>,
}

impl ChatComposer {
  pub(crate) fn new() -> Self {
    Self {
      textarea: TextArea::new(),
      textarea_state: RefCell::new(TextAreaState::default()),
      is_task_running: false,
      footer_mode: FooterMode::ComposerEmpty,
      footer_timer: None,
      paste_burst: PasteBurst::default(),
      command_popup: None,
      history: ChatComposerHistory::new(),
      input_enabled: true,
      steer_enabled: false,
      collaboration_modes_enabled: true,
      collaboration_mode_indicator: None,
      context_window_percent: None,
      context_window_used_tokens: None,
      status_line_value: None,
      status_line_enabled: false,
      local_image_attachments: HashMap::new(),
      remote_image_urls: Vec::new(),
      mention_bindings: Vec::new(),
      pending_pastes: Vec::new(),
    }
  }

  pub(crate) fn set_task_running(&mut self, running: bool) {
    self.is_task_running = running;
  }

  pub(crate) fn set_context_window(&mut self, percent: Option<i64>, used_tokens: Option<i64>) {
    self.context_window_percent = percent;
    self.context_window_used_tokens = used_tokens;
  }

  pub(crate) fn set_status_line(&mut self, status_line: Option<Line<'static>>) {
    self.status_line_value = status_line;
  }

  pub(crate) fn set_status_line_enabled(&mut self, enabled: bool) {
    self.status_line_enabled = enabled;
  }

  pub(crate) fn set_collaboration_mode_indicator(
    &mut self,
    indicator: Option<CollaborationModeIndicator>,
  ) {
    self.collaboration_mode_indicator = indicator;
  }

  pub(crate) fn next_footer_transition_in(&self) -> Option<Duration> {
    let mode = self.footer_mode();
    if !matches!(mode, FooterMode::EscHint | FooterMode::QuitShortcutReminder) {
      return None;
    }
    let started = self.footer_timer?;
    let elapsed = started.elapsed();
    if elapsed >= FOOTER_TRANSIENT_HINT_TIMEOUT {
      return Some(Duration::from_millis(1));
    }
    Some(FOOTER_TRANSIENT_HINT_TIMEOUT.saturating_sub(elapsed))
  }

  pub(crate) fn handle_key_event(&mut self, key: KeyEvent) -> ComposerAction {
    let now = Instant::now();
    self.flush_burst_if_due(now);

    if let Some(popup) = &mut self.command_popup {
      match popup.handle_key(key) {
        CommandPopupAction::Select(cmd) => {
          self.apply_slash_command(cmd);
          self.command_popup = None;
          return ComposerAction::None;
        }
        CommandPopupAction::Dismiss => {
          self.command_popup = None;
          return ComposerAction::None;
        }
        CommandPopupAction::None => {}
      }
    }

    if let KeyCode::Char(ch) = key.code
      && !has_ctrl_or_alt(key.modifiers)
      && !key.modifiers.contains(KeyModifiers::SHIFT)
    {
      let handled = if ch.is_ascii() {
        self.handle_ascii_char(ch, now)
      } else {
        self.handle_non_ascii_char(ch, now)
      };
      if handled {
        self.sync_command_popup();
        self.footer_mode = footer::reset_mode_after_activity(self.footer_mode);
        return ComposerAction::None;
      }
    }

    if !self.input_enabled {
      return ComposerAction::None;
    }

    if let Some(flush_text) = self.paste_burst.flush_before_modified_input()
      && !flush_text.is_empty()
    {
      self.handle_paste(flush_text);
    }

    let action = self.handle_key_event_without_popup(key, now);
    self.paste_burst.clear_window_after_non_char();
    self.sync_command_popup();
    action
  }

  fn handle_key_event_without_popup(&mut self, key: KeyEvent, now: Instant) -> ComposerAction {
    match (key.code, key.modifiers) {
      (KeyCode::Char('c'), mods) if mods.contains(KeyModifiers::CONTROL) => {
        if self.is_task_running {
          return ComposerAction::Interrupt;
        }
        if self.textarea.is_empty() {
          self.footer_mode = FooterMode::QuitShortcutReminder;
          self.footer_timer = Some(now);
          return ComposerAction::RequestQuit;
        }

        self.textarea.set_text_clearing_elements("");
        self.local_image_attachments.clear();
        self.remote_image_urls.clear();
        self.mention_bindings.clear();
        self.pending_pastes.clear();
        self.history.reset_navigation();
        self.footer_mode = FooterMode::ComposerEmpty;
        self.footer_timer = None;
        return ComposerAction::None;
      }
      (KeyCode::Enter, KeyModifiers::NONE) => {
        if self
          .paste_burst
          .newline_should_insert_instead_of_submit(now)
        {
          self.paste_burst.append_newline_if_active(now);
          return ComposerAction::None;
        }
        if self.is_task_running {
          return ComposerAction::Queue;
        }
        return ComposerAction::Submit;
      }
      (KeyCode::Enter, KeyModifiers::SHIFT) => {
        self.textarea.insert_str("\n");
        self.history.reset_navigation();
        self.footer_mode = FooterMode::ComposerHasDraft;
        return ComposerAction::None;
      }
      (KeyCode::Tab, KeyModifiers::NONE) => {
        if self.steer_enabled && self.is_task_running {
          return ComposerAction::Queue;
        }
        if self.steer_enabled {
          return ComposerAction::Submit;
        }
        self.textarea.insert_str("  ");
        self.history.reset_navigation();
        self.footer_mode = FooterMode::ComposerHasDraft;
        return ComposerAction::None;
      }
      (KeyCode::BackTab, _) => {
        self.collaboration_modes_enabled = !self.collaboration_modes_enabled;
        self.footer_mode = footer::reset_mode_after_activity(self.footer_mode);
        return ComposerAction::None;
      }
      (KeyCode::Char('?'), KeyModifiers::NONE) if self.textarea.is_empty() => {
        self.footer_mode =
          footer::toggle_shortcut_mode(self.footer_mode, false, self.textarea.is_empty());
        self.footer_timer = None;
        return ComposerAction::None;
      }
      (KeyCode::Esc, _) => {
        if self.textarea.is_empty() && self.footer_mode() == FooterMode::EscHint {
          self.footer_mode = FooterMode::QuitShortcutReminder;
          self.footer_timer = Some(now);
          return ComposerAction::RequestQuit;
        }
        if self.textarea.is_empty() {
          let next = footer::esc_hint_mode(self.footer_mode, self.is_task_running);
          if next != self.footer_mode {
            self.footer_mode = next;
            self.footer_timer = Some(now);
          }
          return ComposerAction::None;
        }
        self.footer_mode = footer::reset_mode_after_activity(self.footer_mode);
        return ComposerAction::None;
      }
      (KeyCode::Char('k'), mods) if mods.contains(KeyModifiers::CONTROL) => {
        self.textarea.kill_to_end_of_line();
        self.history.reset_navigation();
        return ComposerAction::None;
      }
      (KeyCode::Char('y'), mods) if mods.contains(KeyModifiers::CONTROL) => {
        self.textarea.yank();
        self.history.reset_navigation();
        return ComposerAction::None;
      }
      (KeyCode::Char('w'), mods) if mods.contains(KeyModifiers::CONTROL) => {
        self.textarea.delete_backward_word();
        self.history.reset_navigation();
        return ComposerAction::None;
      }
      (KeyCode::Up, mods) if mods.contains(KeyModifiers::CONTROL) => {
        self.handle_up_key();
        return ComposerAction::None;
      }
      (KeyCode::Down, mods) if mods.contains(KeyModifiers::CONTROL) => {
        self.handle_down_key();
        return ComposerAction::None;
      }
      (KeyCode::Char('/'), KeyModifiers::NONE) if self.textarea.is_empty() => {
        self.textarea.insert_str("/");
        self.command_popup = Some(CommandPopup::new(self.collaboration_modes_enabled));
        self.sync_command_popup();
        return ComposerAction::None;
      }
      _ => {}
    }

    self.textarea.input(key);
    self.history.reset_navigation();
    self.footer_mode = footer::reset_mode_after_activity(self.footer_mode);
    ComposerAction::None
  }

  fn handle_ascii_char(&mut self, ch: char, now: Instant) -> bool {
    match self.paste_burst.on_plain_char(ch, now) {
      CharDecision::RetainFirstChar => true,
      CharDecision::BeginBufferFromPending => {
        self.paste_burst.append_char_to_buffer(ch, now);
        true
      }
      CharDecision::BufferAppend => {
        self.paste_burst.append_char_to_buffer(ch, now);
        true
      }
      CharDecision::BeginBuffer { retro_chars } => {
        let cursor = self.textarea.cursor();
        let before = &self.textarea.text()[..cursor];
        if let Some(grab) = self
          .paste_burst
          .decide_begin_buffer(now, before, retro_chars as usize)
        {
          self.textarea.replace_range(grab.start_byte..cursor, "");
          self.paste_burst.append_char_to_buffer(ch, now);
          return true;
        }
        self.textarea.insert_str(&ch.to_string());
        self.history.reset_navigation();
        false
      }
    }
  }

  fn handle_non_ascii_char(&mut self, ch: char, now: Instant) -> bool {
    let Some(decision) = self.paste_burst.on_plain_char_no_hold(now) else {
      self.textarea.insert_str(&ch.to_string());
      self.history.reset_navigation();
      return false;
    };

    match decision {
      CharDecision::BufferAppend => {
        self.paste_burst.append_char_to_buffer(ch, now);
        true
      }
      CharDecision::BeginBuffer { retro_chars } => {
        let cursor = self.textarea.cursor();
        let before = &self.textarea.text()[..cursor];
        if let Some(grab) = self
          .paste_burst
          .decide_begin_buffer(now, before, retro_chars as usize)
        {
          self.textarea.replace_range(grab.start_byte..cursor, "");
          self.paste_burst.append_char_to_buffer(ch, now);
          return true;
        }
        self.textarea.insert_str(&ch.to_string());
        self.history.reset_navigation();
        false
      }
      CharDecision::RetainFirstChar | CharDecision::BeginBufferFromPending => {
        self.textarea.insert_str(&ch.to_string());
        self.history.reset_navigation();
        false
      }
    }
  }

  /// Flush paste burst if the timeout has elapsed. Returns true if a char or paste was flushed.
  pub(crate) fn flush_burst_if_due(&mut self, now: Instant) -> bool {
    match self.paste_burst.flush_if_due(now) {
      FlushResult::Typed(ch) => {
        self.textarea.insert_str(&ch.to_string());
        self.history.reset_navigation();
        true
      }
      FlushResult::Paste(text) => {
        self.handle_paste(text);
        true
      }
      FlushResult::None => false,
    }
  }

  fn current_history_entry(&self) -> HistoryEntry {
    HistoryEntry {
      text: self.textarea.text().to_string(),
      text_elements: self.textarea.text_elements(),
      local_image_paths: self.local_image_attachments.values().cloned().collect(),
      remote_image_urls: self.remote_image_urls.clone(),
      mention_bindings: self.mention_bindings.clone(),
      pending_pastes: self.pending_pastes.clone(),
    }
  }

  fn apply_history_entry(&mut self, entry: HistoryEntry) {
    self
      .textarea
      .set_text_with_elements(&entry.text, &entry.text_elements);
    self.local_image_attachments = entry
      .local_image_paths
      .iter()
      .enumerate()
      .map(|(idx, path)| (format!("[Image #{}]", idx + 1), path.clone()))
      .collect();
    self.remote_image_urls = entry.remote_image_urls;
    self.mention_bindings = entry.mention_bindings;
    self.pending_pastes = entry.pending_pastes;
    self.footer_mode = if self.textarea.is_empty() {
      FooterMode::ComposerEmpty
    } else {
      FooterMode::ComposerHasDraft
    };
  }

  fn handle_up_key(&mut self) {
    let current_text = self.textarea.text();
    if !self
      .history
      .should_handle_navigation(current_text, self.textarea.cursor())
    {
      self.textarea.move_cursor_up();
      return;
    }
    let current = self.current_history_entry();
    if let Some(prev) = self.history.navigate_prev_entry(current) {
      self.apply_history_entry(prev);
    }
  }

  fn handle_down_key(&mut self) {
    if let Some(next) = self.history.navigate_next_entry() {
      self.apply_history_entry(next);
      return;
    }
    if let Some(saved) = self.history.saved_draft_entry().cloned() {
      self.apply_history_entry(saved);
      return;
    }
    self.textarea.move_cursor_down();
  }

  fn sync_command_popup(&mut self) {
    let text = self.textarea.text();
    if !text.starts_with('/') {
      self.command_popup = None;
      return;
    }
    if self.command_popup.is_none() {
      self.command_popup = Some(CommandPopup::new(self.collaboration_modes_enabled));
    }
    if let Some(popup) = &mut self.command_popup {
      popup.update_filter(text);
    }
  }

  fn apply_slash_command(&mut self, command: SlashCommand) {
    let text = format!("/{} ", command.command());
    self.textarea.set_text_clearing_elements(&text);
    self.footer_mode = footer::reset_mode_after_activity(self.footer_mode);
    self.history.reset_navigation();
  }

  pub(crate) fn handle_paste(&mut self, text: String) {
    self.textarea.insert_str(&text);
    self.history.reset_navigation();
    self.footer_mode = footer::reset_mode_after_activity(self.footer_mode);
    self.paste_burst.clear_after_explicit_paste();
  }

  fn footer_props(&self) -> FooterProps {
    FooterProps {
      mode: self.footer_mode(),
      esc_backtrack_hint: false,
      use_shift_enter_hint: true,
      is_task_running: self.is_task_running,
      steer_enabled: self.steer_enabled,
      collaboration_modes_enabled: self.collaboration_modes_enabled,
      is_wsl: cfg!(target_os = "linux"),
      quit_shortcut_key: key_hint::plain(KeyCode::Esc),
      context_window_percent: self.context_window_percent,
      context_window_used_tokens: self.context_window_used_tokens,
      status_line_value: self.status_line_value.clone(),
      status_line_enabled: self.status_line_enabled,
    }
  }

  fn transient_hint_visible(&self) -> bool {
    self
      .footer_timer
      .is_some_and(|started| started.elapsed() < FOOTER_TRANSIENT_HINT_TIMEOUT)
  }

  fn footer_mode(&self) -> FooterMode {
    let base_mode = if self.textarea.is_empty() {
      FooterMode::ComposerEmpty
    } else {
      FooterMode::ComposerHasDraft
    };

    match self.footer_mode {
      FooterMode::EscHint if self.transient_hint_visible() => FooterMode::EscHint,
      FooterMode::ShortcutOverlay => FooterMode::ShortcutOverlay,
      FooterMode::QuitShortcutReminder if self.transient_hint_visible() => {
        FooterMode::QuitShortcutReminder
      }
      FooterMode::QuitShortcutReminder => base_mode,
      FooterMode::ComposerEmpty | FooterMode::ComposerHasDraft => base_mode,
      FooterMode::EscHint => base_mode,
    }
  }

  pub(crate) fn cursor_pos(&self, composer_area: Rect) -> Option<(u16, u16)> {
    let footer_h = footer::footer_height(&self.footer_props());
    let input_height = composer_area.height.saturating_sub(footer_h);
    let input_area = Rect {
      x: composer_area.x,
      y: composer_area.y,
      width: composer_area.width,
      height: input_height,
    };
    let block = Block::default().borders(Borders::ALL);
    let inner = block.inner(input_area);
    self
      .textarea
      .cursor_pos_with_state(inner, *self.textarea_state.borrow())
  }

  /// Returns whether the composer currently has any paste-burst transient state
  /// (actively buffering, holding a first char for flicker suppression, etc.).
  pub(crate) fn is_in_paste_burst(&self) -> bool {
    self.paste_burst.is_active()
  }

  /// Returns the recommended delay to wait before calling `flush_burst_if_due`,
  /// so that a held first char is reliably flushed out as typed input.
  pub(crate) fn recommended_paste_flush_delay() -> Duration {
    PasteBurst::recommended_flush_delay()
  }

  pub(crate) fn prepare_submission(&mut self) -> Option<ComposerSubmission> {
    let text = self.textarea.text().to_string();
    let text_elements = self.textarea.text_elements();
    if text.trim().is_empty() && self.remote_image_urls.is_empty() {
      return None;
    }

    if !text.trim().is_empty() {
      self.history.push_entry(HistoryEntry {
        text: text.clone(),
        text_elements: text_elements.clone(),
        local_image_paths: self.local_image_attachments.values().cloned().collect(),
        remote_image_urls: self.remote_image_urls.clone(),
        mention_bindings: self.mention_bindings.clone(),
        pending_pastes: self.pending_pastes.clone(),
      });
    }

    self.textarea.set_text_clearing_elements("");
    *self.textarea_state.borrow_mut() = TextAreaState::default();
    self.footer_mode = FooterMode::ComposerEmpty;
    self.footer_timer = None;
    self.command_popup = None;
    self.paste_burst.clear_after_explicit_paste();

    Some(ComposerSubmission {
      text,
      text_elements,
      local_image_attachments: self.local_image_attachments.clone(),
      remote_image_urls: self.remote_image_urls.clone(),
    })
  }
}

impl Renderable for ChatComposer {
  fn render(&self, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
      return;
    }

    let footer_props = self.footer_props();
    let footer_h = footer::footer_height(&footer_props).min(area.height);
    let chunks =
      Layout::vertical([Constraint::Min(1), Constraint::Length(footer_h.max(1))]).split(area);

    let border = if self.input_enabled {
      Block::default().title("Input").borders(Borders::ALL)
    } else {
      Block::default().title("Input".dim()).borders(Borders::ALL)
    };
    let inner = border.inner(chunks[0]);
    border.render(chunks[0], buf);

    if inner.height > 0 {
      StatefulWidgetRef::render_ref(
        &&self.textarea,
        inner,
        buf,
        &mut self.textarea_state.borrow_mut(),
      );
    }

    if let Some(popup) = &self.command_popup {
      let popup_h = inner.height.min(8);
      if popup_h > 0 {
        let popup_area = Rect {
          x: inner.x,
          y: inner.y.saturating_add(inner.height.saturating_sub(popup_h)),
          width: inner.width,
          height: popup_h,
        };
        popup.render(popup_area, buf);
      }
    }

    let show_cycle_hint =
      !footer_props.is_task_running && self.collaboration_mode_indicator.is_some();
    let show_shortcuts_hint =
      matches!(footer_props.mode, FooterMode::ComposerEmpty) && !self.is_in_paste_burst();
    let show_queue_hint = matches!(footer_props.mode, FooterMode::ComposerHasDraft)
      && footer_props.is_task_running
      && footer_props.steer_enabled;

    let right_line = Some(footer::context_window_line(
      footer_props.context_window_percent,
      footer_props.context_window_used_tokens,
    ));
    let right_width = right_line
      .as_ref()
      .map(|line| line.width() as u16)
      .unwrap_or(0);

    if matches!(
      footer_props.mode,
      FooterMode::ComposerEmpty | FooterMode::ComposerHasDraft
    ) {
      let (summary_left, show_context) = footer::single_line_footer_layout(
        chunks[1],
        right_width,
        self.collaboration_mode_indicator,
        show_cycle_hint,
        show_shortcuts_hint,
        show_queue_hint,
      );

      match summary_left {
        footer::SummaryLeft::Default => {
          footer::render_footer_from_props(
            chunks[1],
            buf,
            &footer_props,
            self.collaboration_mode_indicator,
            show_cycle_hint,
            show_shortcuts_hint,
            show_queue_hint,
          );
        }
        footer::SummaryLeft::Custom(line) => {
          footer::render_footer_line(chunks[1], buf, line);
        }
        footer::SummaryLeft::None => {}
      }

      if show_context && let Some(line) = &right_line {
        footer::render_context_right(chunks[1], buf, line);
      }
    } else {
      footer::render_footer_from_props(
        chunks[1],
        buf,
        &footer_props,
        self.collaboration_mode_indicator,
        show_cycle_hint,
        show_shortcuts_hint,
        show_queue_hint,
      );
    }
  }

  fn desired_height(&self, width: u16) -> u16 {
    let textarea_h = self
      .textarea
      .desired_height(width.saturating_sub(2).max(1))
      .clamp(1, 8);
    textarea_h
      .saturating_add(2)
      .saturating_add(footer::footer_height(&self.footer_props()))
  }

  fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
    ChatComposer::cursor_pos(self, area)
  }
}
