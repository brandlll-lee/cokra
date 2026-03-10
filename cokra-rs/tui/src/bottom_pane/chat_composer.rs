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
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::StatefulWidgetRef;
use ratatui::widgets::Widget;

use super::MentionBinding;
use super::chat_composer_history::ChatComposerHistory;
use super::chat_composer_history::HistoryEntry;
use super::command_popup::CommandPopup;
use super::footer;
use super::footer::CollaborationModeIndicator;
use super::footer::FooterMode;
use super::footer::FooterProps;
use super::paste_burst::CharDecision;
use super::paste_burst::FlushResult;
use super::paste_burst::PasteBurst;
use super::slash_commands;
use super::textarea::TextArea;
use super::textarea::TextAreaState;
use crate::key_hint;
use crate::key_hint::has_ctrl_or_alt;
use crate::render::Insets;
use crate::render::RectExt as _;
use crate::render::renderable::Renderable;
use crate::slash_command::SlashCommand;
use crate::style::user_message_style;
use crate::ui_consts::LIVE_PREFIX_COLS;

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
  /// A slash command was selected and should be dispatched by the upper layer.
  SlashCommand(SlashCommand),
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
  // 1:1 codex: placeholder shown when textarea is empty.
  placeholder_text: String,
}

impl ChatComposer {
  pub(crate) fn new() -> Self {
    Self {
      textarea: TextArea::new(),
      textarea_state: RefCell::new(TextAreaState::default()),
      // 1:1 codex: default placeholder text matching codex's input hint.
      placeholder_text:
        "Type @ to mention files, # for issues/PRs, / for commands, or ? for shortcuts".to_string(),
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

  // 1:1 codex: handle_key_event is a clean dispatcher.
  // When a popup is visible, ALL keys route to a popup-specific handler.
  // When no popup, keys route to handle_key_event_without_popup.
  // After every handled key, sync_command_popup() runs.
  pub(crate) fn handle_key_event(&mut self, key: KeyEvent) -> ComposerAction {
    if !self.input_enabled {
      return ComposerAction::None;
    }

    let action = if self.command_popup.is_some() {
      self.handle_key_event_with_slash_popup(key)
    } else {
      self.handle_key_event_without_popup(key)
    };

    // Update (or hide/show) popup after processing the key.
    self.sync_command_popup();

    action
  }

  // 1:1 codex: handle_key_event_with_slash_popup.
  // When the slash-command popup is visible, intercept Up/Down/Esc/Tab/Enter;
  // everything else falls through to handle_input_basic.
  fn handle_key_event_with_slash_popup(&mut self, key: KeyEvent) -> ComposerAction {
    self.footer_mode = footer::reset_mode_after_activity(self.footer_mode);

    match key {
      KeyEvent {
        code: KeyCode::Up, ..
      }
      | KeyEvent {
        code: KeyCode::Char('p'),
        modifiers: KeyModifiers::CONTROL,
        ..
      } => {
        if let Some(popup) = &mut self.command_popup {
          popup.move_up();
        }
        ComposerAction::None
      }
      KeyEvent {
        code: KeyCode::Down,
        ..
      }
      | KeyEvent {
        code: KeyCode::Char('n'),
        modifiers: KeyModifiers::CONTROL,
        ..
      } => {
        if let Some(popup) = &mut self.command_popup {
          popup.move_down();
        }
        ComposerAction::None
      }
      KeyEvent {
        code: KeyCode::Esc, ..
      } => {
        // Dismiss the slash popup; keep the current input untouched.
        self.command_popup = None;
        ComposerAction::None
      }
      KeyEvent {
        code: KeyCode::Tab, ..
      } => {
        // Tab = autocomplete the selected command.
        if let Some(popup) = &mut self.command_popup {
          let first_line = self
            .textarea
            .text()
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
          popup.on_composer_text_change(first_line.clone());
          if let Some(cmd) = popup.selected_command() {
            let starts_with_cmd = first_line
              .trim_start()
              .starts_with(&format!("/{}", cmd.command()));
            if !starts_with_cmd {
              self
                .textarea
                .set_text_clearing_elements(&format!("/{} ", cmd.command()));
            }
            if !self.textarea.text().is_empty() {
              self.textarea.set_cursor(self.textarea.text().len());
            }
          }
        }
        ComposerAction::None
      }
      KeyEvent {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::NONE,
        ..
      } => {
        // Enter = execute the selected command.
        if let Some(popup) = &self.command_popup
          && let Some(cmd) = popup.selected_command()
        {
          self.command_popup = None;
          return self.apply_slash_command(cmd);
        }
        // Fallback to default submit handling if no command selected.
        self.command_popup = None;
        self.handle_key_event_without_popup(key)
      }
      // All other keys: fall through to handle_input_basic (paste burst + char insert).
      input => self.handle_input_basic(input),
    }
  }

  // 1:1 codex: handle_key_event_without_popup.
  fn handle_key_event_without_popup(&mut self, key: KeyEvent) -> ComposerAction {
    let now = Instant::now();
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
        ComposerAction::None
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
        ComposerAction::Submit
      }
      (KeyCode::Enter, KeyModifiers::SHIFT) => {
        self.textarea.insert_str("\n");
        self.history.reset_navigation();
        self.footer_mode = FooterMode::ComposerHasDraft;
        ComposerAction::None
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
        ComposerAction::None
      }
      (KeyCode::BackTab, _) => {
        self.collaboration_modes_enabled = !self.collaboration_modes_enabled;
        self.footer_mode = footer::reset_mode_after_activity(self.footer_mode);
        ComposerAction::None
      }
      (KeyCode::Char('?'), KeyModifiers::NONE) if self.textarea.is_empty() => {
        self.footer_mode =
          footer::toggle_shortcut_mode(self.footer_mode, false, self.textarea.is_empty());
        self.footer_timer = None;
        ComposerAction::None
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
        ComposerAction::None
      }
      (KeyCode::Char('k'), mods) if mods.contains(KeyModifiers::CONTROL) => {
        self.textarea.kill_to_end_of_line();
        self.history.reset_navigation();
        ComposerAction::None
      }
      (KeyCode::Char('y'), mods) if mods.contains(KeyModifiers::CONTROL) => {
        self.textarea.yank();
        self.history.reset_navigation();
        ComposerAction::None
      }
      (KeyCode::Char('w'), mods) if mods.contains(KeyModifiers::CONTROL) => {
        self.textarea.delete_backward_word();
        self.history.reset_navigation();
        ComposerAction::None
      }
      // History navigation
      (KeyCode::Up, _) | (KeyCode::Down, _) => {
        if self
          .history
          .should_handle_navigation(self.textarea.text(), self.textarea.cursor())
        {
          match key.code {
            KeyCode::Up => self.handle_up_key(),
            KeyCode::Down => self.handle_down_key(),
            _ => unreachable!(),
          }
          return ComposerAction::None;
        }
        self.handle_input_basic(key)
      }
      // Everything else -> handle_input_basic (paste burst + char insert).
      _ => self.handle_input_basic(key),
    }
  }

  // 1:1 codex: handle_input_basic.
  // Handles keys that mutate the textarea, including paste-burst detection.
  // This is the lowest-level keypath for keys that mutate the textarea.
  fn handle_input_basic(&mut self, input: KeyEvent) -> ComposerAction {
    let now = Instant::now();

    // Always flush any *due* paste burst first.
    self.flush_burst_if_due(now);

    if !matches!(input.code, KeyCode::Esc) {
      self.footer_mode = footer::reset_mode_after_activity(self.footer_mode);
    }

    // If we're capturing a burst and receive Enter, accumulate it.
    if matches!(input.code, KeyCode::Enter)
      && self.paste_burst.is_active()
      && self.paste_burst.append_newline_if_active(now)
    {
      return ComposerAction::None;
    }

    // Intercept plain Char inputs to optionally accumulate into a burst buffer.
    if let KeyEvent {
      code: KeyCode::Char(ch),
      modifiers,
      ..
    } = input
    {
      let has_ctrl_or_alt = has_ctrl_or_alt(modifiers);
      if !has_ctrl_or_alt {
        // Non-ASCII characters: handle via handle_non_ascii_char (IME path).
        if !ch.is_ascii() {
          return self.handle_non_ascii_char_action(ch, now);
        }

        // ASCII characters: paste burst detection.
        match self.paste_burst.on_plain_char(ch, now) {
          CharDecision::BufferAppend => {
            self.paste_burst.append_char_to_buffer(ch, now);
            return ComposerAction::None;
          }
          CharDecision::BeginBuffer { retro_chars } => {
            let cursor = self.textarea.cursor();
            let txt = self.textarea.text();
            let safe_cur = Self::clamp_to_char_boundary(txt, cursor);
            let before = &txt[..safe_cur];
            if let Some(grab) =
              self
                .paste_burst
                .decide_begin_buffer(now, before, retro_chars as usize)
            {
              if !grab.grabbed.is_empty() {
                self.textarea.replace_range(grab.start_byte..safe_cur, "");
              }
              self.paste_burst.append_char_to_buffer(ch, now);
              return ComposerAction::None;
            }
            // Fall through to normal insertion below.
          }
          CharDecision::BeginBufferFromPending => {
            self.paste_burst.append_char_to_buffer(ch, now);
            return ComposerAction::None;
          }
          CharDecision::RetainFirstChar => {
            return ComposerAction::None;
          }
        }
      }
      if let Some(pasted) = self.paste_burst.flush_before_modified_input() {
        self.handle_paste(pasted);
      }
    }

    // Flush any buffered burst before applying a non-char input.
    if !matches!(input.code, KeyCode::Char(_) | KeyCode::Enter)
      && let Some(pasted) = self.paste_burst.flush_before_modified_input()
    {
      self.handle_paste(pasted);
    }

    // For non-char inputs (or after flushing), handle normally.
    self.textarea.input(input);
    self.history.reset_navigation();

    // Update paste-burst heuristic.
    match input.code {
      KeyCode::Char(_) => {
        if has_ctrl_or_alt(input.modifiers) {
          self.paste_burst.clear_window_after_non_char();
        }
      }
      KeyCode::Enter => {
        // Keep burst window alive (supports blank lines in paste).
      }
      _ => {
        self.paste_burst.clear_window_after_non_char();
      }
    }

    ComposerAction::None
  }

  #[inline]
  fn clamp_to_char_boundary(text: &str, pos: usize) -> usize {
    let mut p = pos.min(text.len());
    if p < text.len() && !text.is_char_boundary(p) {
      p = text
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= p)
        .last()
        .unwrap_or(0);
    }
    p
  }

  // 1:1 codex: handle_non_ascii_char (IME path).
  // Non-ASCII input (e.g. Chinese) never holds the first char, but still
  // supports paste-burst detection. Returns ComposerAction directly.
  fn handle_non_ascii_char_action(&mut self, ch: char, now: Instant) -> ComposerAction {
    if self.paste_burst.try_append_char_if_active(ch, now) {
      return ComposerAction::None;
    }
    // Flush any existing burst buffer before applying non-ASCII input.
    if let Some(pasted) = self.paste_burst.flush_before_modified_input() {
      self.handle_paste(pasted);
    }
    if let Some(decision) = self.paste_burst.on_plain_char_no_hold(now) {
      match decision {
        CharDecision::BufferAppend => {
          self.paste_burst.append_char_to_buffer(ch, now);
          return ComposerAction::None;
        }
        CharDecision::BeginBuffer { retro_chars } => {
          let cur = self.textarea.cursor();
          let txt = self.textarea.text();
          let safe_cur = Self::clamp_to_char_boundary(txt, cur);
          let before = &txt[..safe_cur];
          if let Some(grab) =
            self
              .paste_burst
              .decide_begin_buffer(now, before, retro_chars as usize)
          {
            if !grab.grabbed.is_empty() {
              self.textarea.replace_range(grab.start_byte..safe_cur, "");
            }
            self.paste_burst.append_char_to_buffer(ch, now);
            return ComposerAction::None;
          }
          // Fall through to normal insertion.
        }
        _ => unreachable!("on_plain_char_no_hold returned unexpected variant"),
      }
    }
    if let Some(pasted) = self.paste_burst.flush_before_modified_input() {
      self.handle_paste(pasted);
    }
    self.textarea.insert_str(&ch.to_string());
    self.history.reset_navigation();
    ComposerAction::None
  }

  /// Flush paste burst if the timeout has elapsed. Returns true if a char or paste was flushed.
  pub(crate) fn flush_burst_if_due(&mut self, now: Instant) -> bool {
    match self.paste_burst.flush_if_due(now) {
      FlushResult::Typed(ch) => {
        self.textarea.insert_str(&ch.to_string());
        self.history.reset_navigation();
        // Mirror codex: sync popups when a pending fast char flushes as
        // normal typed input so slash-command popup appears for '/'.
        self.sync_command_popup();
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

  // 1:1 codex: sync_command_popup (codex calls this sync_popups → sync_command_popup).
  // Determines whether the caret is inside the initial '/name' token on the
  // first line. If so, shows/updates the popup; otherwise hides it.
  fn sync_command_popup(&mut self) {
    let text = self.textarea.text();
    let first_line_end = text.find('\n').unwrap_or(text.len());
    let first_line = &text[..first_line_end];
    let cursor = self.textarea.cursor();
    let caret_on_first_line = cursor <= first_line_end;

    // Check if the cursor is currently within a slash command name.
    let is_editing_slash_command = caret_on_first_line
      && first_line.starts_with('/')
      && self.looks_like_slash_prefix(first_line, cursor);

    match &mut self.command_popup {
      Some(popup) => {
        if is_editing_slash_command {
          popup.on_composer_text_change(first_line.to_string());
        } else {
          self.command_popup = None;
        }
      }
      None => {
        if is_editing_slash_command {
          let mut popup = CommandPopup::new(self.collaboration_modes_enabled);
          popup.on_composer_text_change(first_line.to_string());
          self.command_popup = Some(popup);
        }
      }
    }
  }

  // 1:1 codex: looks_like_slash_prefix + slash_command_under_cursor.
  // Checks if the cursor is positioned within the '/name' portion of a
  // slash command and the name looks like a valid prefix.
  fn looks_like_slash_prefix(&self, first_line: &str, cursor: usize) -> bool {
    if !first_line.starts_with('/') {
      return false;
    }
    let name_start = 1usize;
    let name_end = first_line[name_start..]
      .find(char::is_whitespace)
      .map(|idx| name_start + idx)
      .unwrap_or(first_line.len());

    // Cursor must be within the /name token.
    if cursor > name_end {
      return false;
    }

    let name = &first_line[name_start..name_end];
    // Empty name only valid when there is nothing else after '/'.
    if name.is_empty() {
      return name_end == first_line.len();
    }

    slash_commands::has_builtin_prefix(name, self.collaboration_modes_enabled, true, true, false)
  }

  fn apply_slash_command(&mut self, command: SlashCommand) -> ComposerAction {
    // Clear the textarea and bubble the command up to the app layer.
    self.textarea.set_text_clearing_elements("");
    self.footer_mode = footer::reset_mode_after_activity(self.footer_mode);
    self.history.reset_navigation();
    ComposerAction::SlashCommand(command)
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

  // 1:1 codex: cursor_pos uses the same layout_areas logic as render.
  pub(crate) fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
    let footer_h = footer::footer_height(&self.footer_props());
    let popup_constraint = if let Some(popup) = &self.command_popup {
      Constraint::Max(popup.calculate_required_height(area.width))
    } else {
      Constraint::Max(footer_h)
    };
    let [composer_rect, _popup_rect] =
      Layout::vertical([Constraint::Min(3), popup_constraint]).areas(area);
    // 1:1 codex: Insets(top=1, left=LIVE_PREFIX_COLS, bottom=1, right=1) instead of Block border.
    let textarea_rect = composer_rect.inset(Insets::tlbr(1, LIVE_PREFIX_COLS, 1, 1));
    self
      .textarea
      .cursor_pos_with_state(textarea_rect, *self.textarea_state.borrow())
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

  pub(crate) fn has_command_popup(&self) -> bool {
    self.command_popup.is_some()
  }

  pub(crate) fn can_focus_status_line_selector(&self) -> bool {
    self.textarea.text().is_empty()
      && self.local_image_attachments.is_empty()
      && self.remote_image_urls.is_empty()
      && self.pending_pastes.is_empty()
      && !self.paste_burst.is_active()
  }
}

impl Renderable for ChatComposer {
  // 1:1 codex: render_with_mask.
  // Layout: [composer_rect (background fill + › prefix + textarea)] then [popup_rect (popup OR footer)].
  // Popup and footer share the SAME slot below the composer.
  fn render(&self, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
      return;
    }

    let footer_props = self.footer_props();
    let footer_h = footer::footer_height(&footer_props);

    // 1:1 codex layout_areas: popup and footer share the bottom slot.
    let popup_constraint = if let Some(popup) = &self.command_popup {
      Constraint::Max(popup.calculate_required_height(area.width))
    } else {
      Constraint::Max(footer_h)
    };
    let [composer_rect, popup_rect] =
      Layout::vertical([Constraint::Min(3), popup_constraint]).areas(area);

    // 1:1 codex: Insets(top=1, left=LIVE_PREFIX_COLS, bottom=1, right=1) — no border, just spacing.
    let textarea_rect = composer_rect.inset(Insets::tlbr(1, LIVE_PREFIX_COLS, 1, 1));

    // 1:1 codex: fill composer_rect with user_message_style (subtle background tint, no border).
    let style = user_message_style();
    Block::default().style(style).render(composer_rect, buf);

    // 1:1 codex: render the › prompt at the left gutter.
    if !textarea_rect.is_empty() {
      let prompt = if self.input_enabled {
        "›".bold()
      } else {
        "›".dim()
      };
      buf.set_span(
        textarea_rect.x.saturating_sub(LIVE_PREFIX_COLS),
        textarea_rect.y,
        &prompt,
        textarea_rect.width,
      );
    }

    // 1:1 codex: render the textarea content.
    if textarea_rect.height > 0 {
      StatefulWidgetRef::render_ref(
        &&self.textarea,
        textarea_rect,
        buf,
        &mut self.textarea_state.borrow_mut(),
      );
    }

    // 1:1 codex: render placeholder text when textarea is empty.
    if self.textarea.text().is_empty() && !textarea_rect.is_empty() {
      let text = if self.input_enabled {
        self.placeholder_text.as_str()
      } else {
        "Input disabled."
      };
      let placeholder = Span::from(text.to_string()).dim();
      Line::from(vec![placeholder]).render(textarea_rect, buf);
    }

    // Render popup OR footer in the shared bottom slot.
    if let Some(popup) = &self.command_popup {
      popup.render(popup_rect, buf);
    } else {
      // Footer rendering.
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
          popup_rect,
          right_width,
          self.collaboration_mode_indicator,
          show_cycle_hint,
          show_shortcuts_hint,
          show_queue_hint,
        );

        match summary_left {
          footer::SummaryLeft::Default => {
            footer::render_footer_from_props(
              popup_rect,
              buf,
              &footer_props,
              self.collaboration_mode_indicator,
              show_cycle_hint,
              show_shortcuts_hint,
              show_queue_hint,
            );
          }
          footer::SummaryLeft::Custom(line) => {
            footer::render_footer_line(popup_rect, buf, line);
          }
          footer::SummaryLeft::None => {}
        }

        if show_context && let Some(line) = &right_line {
          footer::render_context_right(popup_rect, buf, line);
        }
      } else {
        footer::render_footer_from_props(
          popup_rect,
          buf,
          &footer_props,
          self.collaboration_mode_indicator,
          show_cycle_hint,
          show_shortcuts_hint,
          show_queue_hint,
        );
      }
    }
  }

  // 1:1 codex: desired_height uses Insets (top=1, bottom=1) instead of border (top+bottom=2).
  // The effective overhead is the same (2 rows) but semantics match codex layout_areas.
  fn desired_height(&self, width: u16) -> u16 {
    // 1:1 codex: inner_width = total_width - LIVE_PREFIX_COLS - right_margin(1)
    const COLS_WITH_MARGIN: u16 = LIVE_PREFIX_COLS + 1;
    let inner_width = width.saturating_sub(COLS_WITH_MARGIN).max(1);
    let textarea_h = self.textarea.desired_height(inner_width).clamp(1, 8);
    let popup_or_footer_h = if let Some(popup) = &self.command_popup {
      popup.calculate_required_height(width)
    } else {
      footer::footer_height(&self.footer_props())
    };
    textarea_h
      .saturating_add(2) // top inset (1) + bottom inset (1)
      .saturating_add(popup_or_footer_h)
  }

  fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
    ChatComposer::cursor_pos(self, area)
  }
}
