use crate::key_hint::is_altgr;
use cokra_protocol::user_input::ByteRange;
use cokra_protocol::user_input::TextElement as UserTextElement;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::widgets::StatefulWidgetRef;
use ratatui::widgets::WidgetRef;
use std::cell::Ref;
use std::cell::RefCell;
use std::ops::Range;
use textwrap::Options;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const WORD_SEPARATORS: &str = "`~!@#$%^&*()-=+[{]}\\|;:'\",.<>/?";

fn is_word_separator(ch: char) -> bool {
  WORD_SEPARATORS.contains(ch)
}

#[derive(Debug, Clone)]
struct TextElement {
  id: u64,
  range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextElementSnapshot {
  pub(crate) id: u64,
  pub(crate) range: Range<usize>,
  pub(crate) text: String,
}

#[derive(Debug)]
pub(crate) struct TextArea {
  text: String,
  cursor_pos: usize,
  wrap_cache: RefCell<Option<WrapCache>>,
  preferred_col: Option<usize>,
  elements: Vec<TextElement>,
  next_element_id: u64,
  kill_buffer: String,
}

#[derive(Debug, Clone)]
struct WrapCache {
  width: u16,
  lines: Vec<Range<usize>>,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct TextAreaState {
  /// Index into wrapped lines of the first visible line.
  scroll: u16,
}

impl TextArea {
  pub fn new() -> Self {
    Self {
      text: String::new(),
      cursor_pos: 0,
      wrap_cache: RefCell::new(None),
      preferred_col: None,
      elements: Vec::new(),
      next_element_id: 1,
      kill_buffer: String::new(),
    }
  }

  /// Replace the textarea text and clear any existing text elements.
  pub fn set_text_clearing_elements(&mut self, text: &str) {
    self.set_text_inner(text, None);
  }

  /// Replace the textarea text and set the provided text elements.
  pub fn set_text_with_elements(&mut self, text: &str, elements: &[UserTextElement]) {
    self.set_text_inner(text, Some(elements));
  }

  fn set_text_inner(&mut self, text: &str, elements: Option<&[UserTextElement]>) {
    // Stage 1: replace the raw text and keep the cursor in a safe byte range.
    self.text = text.to_string();
    self.cursor_pos = self.cursor_pos.clamp(0, self.text.len());
    // Stage 2: rebuild element ranges from scratch against the new text.
    self.elements.clear();
    if let Some(elements) = elements {
      for elem in elements {
        let mut start = elem.byte_range.start.min(self.text.len());
        let mut end = elem.byte_range.end.min(self.text.len());
        start = self.clamp_pos_to_char_boundary(start);
        end = self.clamp_pos_to_char_boundary(end);
        if start >= end {
          continue;
        }
        let id = self.next_element_id();
        self.elements.push(TextElement {
          id,
          range: start..end,
        });
      }
      self.elements.sort_by_key(|e| e.range.start);
    }
    // Stage 3: clamp the cursor and reset derived state tied to the prior content.
    self.cursor_pos = self.clamp_pos_to_nearest_boundary(self.cursor_pos);
    self.wrap_cache.replace(None);
    self.preferred_col = None;
    self.kill_buffer.clear();
  }

  pub fn text(&self) -> &str {
    &self.text
  }

  pub fn insert_str(&mut self, text: &str) {
    self.insert_str_at(self.cursor_pos, text);
  }

  pub fn insert_str_at(&mut self, pos: usize, text: &str) {
    let pos = self.clamp_pos_for_insertion(pos);
    self.text.insert_str(pos, text);
    self.wrap_cache.replace(None);
    if pos <= self.cursor_pos {
      self.cursor_pos += text.len();
    }
    self.shift_elements(pos, 0, text.len());
    self.preferred_col = None;
  }

  pub fn replace_range(&mut self, range: std::ops::Range<usize>, text: &str) {
    let range = self.expand_range_to_element_boundaries(range);
    self.replace_range_raw(range, text);
  }

  fn replace_range_raw(&mut self, range: std::ops::Range<usize>, text: &str) {
    assert!(range.start <= range.end);
    let start = range.start.clamp(0, self.text.len());
    let end = range.end.clamp(0, self.text.len());
    let removed_len = end - start;
    let inserted_len = text.len();
    if removed_len == 0 && inserted_len == 0 {
      return;
    }
    let diff = inserted_len as isize - removed_len as isize;

    self.text.replace_range(range, text);
    self.wrap_cache.replace(None);
    self.preferred_col = None;
    self.update_elements_after_replace(start, end, inserted_len);

    // Update the cursor position to account for the edit.
    self.cursor_pos = if self.cursor_pos < start {
      // Cursor was before the edited range – no shift.
      self.cursor_pos
    } else if self.cursor_pos <= end {
      // Cursor was inside the replaced range – move to end of the new text.
      start + inserted_len
    } else {
      // Cursor was after the replaced range – shift by the length diff.
      ((self.cursor_pos as isize) + diff) as usize
    }
    .min(self.text.len());

    // Ensure cursor is not inside an element
    self.cursor_pos = self.clamp_pos_to_nearest_boundary(self.cursor_pos);
  }

  pub fn cursor(&self) -> usize {
    self.cursor_pos
  }

  pub fn set_cursor(&mut self, pos: usize) {
    self.cursor_pos = pos.clamp(0, self.text.len());
    self.cursor_pos = self.clamp_pos_to_nearest_boundary(self.cursor_pos);
    self.preferred_col = None;
  }

  pub fn desired_height(&self, width: u16) -> u16 {
    self.wrapped_lines(width).len() as u16
  }

  #[cfg_attr(not(test), allow(dead_code))]
  pub fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
    self.cursor_pos_with_state(area, TextAreaState::default())
  }

  /// Compute the on-screen cursor position taking scrolling into account.
  pub fn cursor_pos_with_state(&self, area: Rect, state: TextAreaState) -> Option<(u16, u16)> {
    let lines = self.wrapped_lines(area.width);
    let effective_scroll = self.effective_scroll(area.height, &lines, state.scroll);
    let i = Self::wrapped_line_index_by_start(&lines, self.cursor_pos)?;
    let ls = &lines[i];
    let col = self.text[ls.start..self.cursor_pos].width() as u16;
    let screen_row = i
      .saturating_sub(effective_scroll as usize)
      .try_into()
      .unwrap_or(0);
    Some((area.x + col, area.y + screen_row))
  }

  pub fn is_empty(&self) -> bool {
    self.text.is_empty()
  }

  fn current_display_col(&self) -> usize {
    let bol = self.beginning_of_current_line();
    self.text[bol..self.cursor_pos].width()
  }

  fn wrapped_line_index_by_start(lines: &[Range<usize>], pos: usize) -> Option<usize> {
    // partition_point returns the index of the first element for which
    // the predicate is false, i.e. the count of elements with start <= pos.
    let idx = lines.partition_point(|r| r.start <= pos);
    if idx == 0 { None } else { Some(idx - 1) }
  }

  fn move_to_display_col_on_line(&mut self, line_start: usize, line_end: usize, target_col: usize) {
    let mut width_so_far = 0usize;
    for (i, g) in self.text[line_start..line_end].grapheme_indices(true) {
      width_so_far += g.width();
      if width_so_far > target_col {
        self.cursor_pos = line_start + i;
        // Avoid landing inside an element; round to nearest boundary
        self.cursor_pos = self.clamp_pos_to_nearest_boundary(self.cursor_pos);
        return;
      }
    }
    self.cursor_pos = line_end;
    self.cursor_pos = self.clamp_pos_to_nearest_boundary(self.cursor_pos);
  }

  fn beginning_of_line(&self, pos: usize) -> usize {
    self.text[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0)
  }
  fn beginning_of_current_line(&self) -> usize {
    self.beginning_of_line(self.cursor_pos)
  }

  fn end_of_line(&self, pos: usize) -> usize {
    self.text[pos..]
      .find('\n')
      .map(|i| i + pos)
      .unwrap_or(self.text.len())
  }
  fn end_of_current_line(&self) -> usize {
    self.end_of_line(self.cursor_pos)
  }

  pub fn input(&mut self, event: KeyEvent) {
    match event {
            // Some terminals (or configurations) send Control key chords as
            // C0 control characters without reporting the CONTROL modifier.
            // Handle common fallbacks for Ctrl-B/F/P/N here so they don't get
            // inserted as literal control bytes.
            KeyEvent { code: KeyCode::Char('\u{0002}'), modifiers: KeyModifiers::NONE, .. } /* ^B */ => {
                self.move_cursor_left();
            }
            KeyEvent { code: KeyCode::Char('\u{0006}'), modifiers: KeyModifiers::NONE, .. } /* ^F */ => {
                self.move_cursor_right();
            }
            KeyEvent { code: KeyCode::Char('\u{0010}'), modifiers: KeyModifiers::NONE, .. } /* ^P */ => {
                self.move_cursor_up();
            }
            KeyEvent { code: KeyCode::Char('\u{000e}'), modifiers: KeyModifiers::NONE, .. } /* ^N */ => {
                self.move_cursor_down();
            }
            KeyEvent {
                code: KeyCode::Char(c),
                // Insert plain characters (and Shift-modified). Do NOT insert when ALT is held,
                // because many terminals map Option/Meta combos to ALT+<char> (e.g. ESC f/ESC b)
                // for word navigation. Those are handled explicitly below.
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
                ..
            } => self.insert_str(&c.to_string()),
            KeyEvent {
                code: KeyCode::Char('j' | 'm'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }
            | KeyEvent {
                code: KeyCode::Enter,
                ..
            } => self.insert_str("\n"),
            KeyEvent {
                code: KeyCode::Char('h'),
                modifiers,
                ..
            } if modifiers == (KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.delete_backward_word()
            },
            // Windows AltGr generates ALT|CONTROL; treat as a plain character input unless
            // we match a specific Control+Alt binding above.
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            } if is_altgr(modifiers) => self.insert_str(&c.to_string()),
            KeyEvent {
                code: KeyCode::Backspace,
                modifiers: KeyModifiers::ALT,
                ..
            } => self.delete_backward_word(),
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('h'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => self.delete_backward(1),
            KeyEvent {
                code: KeyCode::Delete,
                modifiers: KeyModifiers::ALT,
                ..
            }  => self.delete_forward_word(),
            KeyEvent {
                code: KeyCode::Delete,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('d'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => self.delete_forward(1),

            KeyEvent {
                code: KeyCode::Char('w'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.delete_backward_word();
            }
            // Meta-b -> move to beginning of previous word
            // Meta-f -> move to end of next word
            // Many terminals map Option (macOS) to Alt. Some send Alt|Shift, so match contains(ALT).
            KeyEvent {
                code: KeyCode::Char('b'),
                modifiers: KeyModifiers::ALT,
                ..
            } => {
                self.set_cursor(self.beginning_of_previous_word());
            }
            KeyEvent {
                code: KeyCode::Char('f'),
                modifiers: KeyModifiers::ALT,
                ..
            } => {
                self.set_cursor(self.end_of_next_word());
            }
            KeyEvent {
                code: KeyCode::Char('u'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.kill_to_beginning_of_line();
            }
            KeyEvent {
                code: KeyCode::Char('k'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.kill_to_end_of_line();
            }
            KeyEvent {
                code: KeyCode::Char('y'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.yank();
            }

            // Cursor movement
            KeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.move_cursor_left();
            }
            KeyEvent {
                code: KeyCode::Right,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.move_cursor_right();
            }
            KeyEvent {
                code: KeyCode::Char('b'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.move_cursor_left();
            }
            KeyEvent {
                code: KeyCode::Char('f'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.move_cursor_right();
            }
            KeyEvent {
                code: KeyCode::Char('p'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.move_cursor_up();
            }
            KeyEvent {
                code: KeyCode::Char('n'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.move_cursor_down();
            }
            // Some terminals send Alt+Arrow for word-wise movement:
            // Option/Left -> Alt+Left (previous word start)
            // Option/Right -> Alt+Right (next word end)
            KeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::ALT,
                ..
            }
            | KeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.set_cursor(self.beginning_of_previous_word());
            }
            KeyEvent {
                code: KeyCode::Right,
                modifiers: KeyModifiers::ALT,
                ..
            }
            | KeyEvent {
                code: KeyCode::Right,
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.set_cursor(self.end_of_next_word());
            }
            KeyEvent {
                code: KeyCode::Up, ..
            } => {
                self.move_cursor_up();
            }
            KeyEvent {
                code: KeyCode::Down,
                ..
            } => {
                self.move_cursor_down();
            }
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => {
                self.move_cursor_to_beginning_of_line(false);
            }
            KeyEvent {
                code: KeyCode::Char('a'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.move_cursor_to_beginning_of_line(true);
            }

            KeyEvent {
                code: KeyCode::End, ..
            } => {
                self.move_cursor_to_end_of_line(false);
            }
            KeyEvent {
                code: KeyCode::Char('e'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.move_cursor_to_end_of_line(true);
            }
            _o => {
                #[cfg(feature = "debug-logs")]
                tracing::debug!("Unhandled key event in TextArea: {:?}", _o);
            }
        }
  }

  // ####### Input Functions #######
  pub fn delete_backward(&mut self, n: usize) {
    if n == 0 || self.cursor_pos == 0 {
      return;
    }
    let mut target = self.cursor_pos;
    for _ in 0..n {
      target = self.prev_atomic_boundary(target);
      if target == 0 {
        break;
      }
    }
    self.replace_range(target..self.cursor_pos, "");
  }

  pub fn delete_forward(&mut self, n: usize) {
    if n == 0 || self.cursor_pos >= self.text.len() {
      return;
    }
    let mut target = self.cursor_pos;
    for _ in 0..n {
      target = self.next_atomic_boundary(target);
      if target >= self.text.len() {
        break;
      }
    }
    self.replace_range(self.cursor_pos..target, "");
  }

  pub fn delete_backward_word(&mut self) {
    let start = self.beginning_of_previous_word();
    self.kill_range(start..self.cursor_pos);
  }

  /// Delete text to the right of the cursor using "word" semantics.
  ///
  /// Deletes from the current cursor position through the end of the next word as determined
  /// by `end_of_next_word()`. Any whitespace (including newlines) between the cursor and that
  /// word is included in the deletion.
  pub fn delete_forward_word(&mut self) {
    let end = self.end_of_next_word();
    if end > self.cursor_pos {
      self.kill_range(self.cursor_pos..end);
    }
  }

  pub fn kill_to_end_of_line(&mut self) {
    let eol = self.end_of_current_line();
    let range = if self.cursor_pos == eol {
      if eol < self.text.len() {
        Some(self.cursor_pos..eol + 1)
      } else {
        None
      }
    } else {
      Some(self.cursor_pos..eol)
    };

    if let Some(range) = range {
      self.kill_range(range);
    }
  }

  pub fn kill_to_beginning_of_line(&mut self) {
    let bol = self.beginning_of_current_line();
    let range = if self.cursor_pos == bol {
      if bol > 0 { Some(bol - 1..bol) } else { None }
    } else {
      Some(bol..self.cursor_pos)
    };

    if let Some(range) = range {
      self.kill_range(range);
    }
  }

  pub fn yank(&mut self) {
    if self.kill_buffer.is_empty() {
      return;
    }
    let text = self.kill_buffer.clone();
    self.insert_str(&text);
  }

  fn kill_range(&mut self, range: Range<usize>) {
    let range = self.expand_range_to_element_boundaries(range);
    if range.start >= range.end {
      return;
    }

    let removed = self.text[range.clone()].to_string();
    if removed.is_empty() {
      return;
    }

    self.kill_buffer = removed;
    self.replace_range_raw(range, "");
  }

  /// Move the cursor left by a single grapheme cluster.
  pub fn move_cursor_left(&mut self) {
    self.cursor_pos = self.prev_atomic_boundary(self.cursor_pos);
    self.preferred_col = None;
  }

  /// Move the cursor right by a single grapheme cluster.
  pub fn move_cursor_right(&mut self) {
    self.cursor_pos = self.next_atomic_boundary(self.cursor_pos);
    self.preferred_col = None;
  }

  pub fn move_cursor_up(&mut self) {
    // If we have a wrapping cache, prefer navigating across wrapped (visual) lines.
    if let Some((target_col, maybe_line)) = {
      let cache_ref = self.wrap_cache.borrow();
      if let Some(cache) = cache_ref.as_ref() {
        let lines = &cache.lines;
        if let Some(idx) = Self::wrapped_line_index_by_start(lines, self.cursor_pos) {
          let cur_range = &lines[idx];
          let target_col = self
            .preferred_col
            .unwrap_or_else(|| self.text[cur_range.start..self.cursor_pos].width());
          if idx > 0 {
            let prev = &lines[idx - 1];
            let line_start = prev.start;
            let line_end = prev.end.saturating_sub(1);
            Some((target_col, Some((line_start, line_end))))
          } else {
            Some((target_col, None))
          }
        } else {
          None
        }
      } else {
        None
      }
    } {
      // We had wrapping info. Apply movement accordingly.
      match maybe_line {
        Some((line_start, line_end)) => {
          if self.preferred_col.is_none() {
            self.preferred_col = Some(target_col);
          }
          self.move_to_display_col_on_line(line_start, line_end, target_col);
          return;
        }
        None => {
          // Already at first visual line -> move to start
          self.cursor_pos = 0;
          self.preferred_col = None;
          return;
        }
      }
    }

    // Fallback to logical line navigation if we don't have wrapping info yet.
    if let Some(prev_nl) = self.text[..self.cursor_pos].rfind('\n') {
      let target_col = match self.preferred_col {
        Some(c) => c,
        None => {
          let c = self.current_display_col();
          self.preferred_col = Some(c);
          c
        }
      };
      let prev_line_start = self.text[..prev_nl].rfind('\n').map(|i| i + 1).unwrap_or(0);
      let prev_line_end = prev_nl;
      self.move_to_display_col_on_line(prev_line_start, prev_line_end, target_col);
    } else {
      self.cursor_pos = 0;
      self.preferred_col = None;
    }
  }

  pub fn move_cursor_down(&mut self) {
    // If we have a wrapping cache, prefer navigating across wrapped (visual) lines.
    if let Some((target_col, move_to_last)) = {
      let cache_ref = self.wrap_cache.borrow();
      if let Some(cache) = cache_ref.as_ref() {
        let lines = &cache.lines;
        if let Some(idx) = Self::wrapped_line_index_by_start(lines, self.cursor_pos) {
          let cur_range = &lines[idx];
          let target_col = self
            .preferred_col
            .unwrap_or_else(|| self.text[cur_range.start..self.cursor_pos].width());
          if idx + 1 < lines.len() {
            let next = &lines[idx + 1];
            let line_start = next.start;
            let line_end = next.end.saturating_sub(1);
            Some((target_col, Some((line_start, line_end))))
          } else {
            Some((target_col, None))
          }
        } else {
          None
        }
      } else {
        None
      }
    } {
      match move_to_last {
        Some((line_start, line_end)) => {
          if self.preferred_col.is_none() {
            self.preferred_col = Some(target_col);
          }
          self.move_to_display_col_on_line(line_start, line_end, target_col);
          return;
        }
        None => {
          // Already on last visual line -> move to end
          self.cursor_pos = self.text.len();
          self.preferred_col = None;
          return;
        }
      }
    }

    // Fallback to logical line navigation if we don't have wrapping info yet.
    let target_col = match self.preferred_col {
      Some(c) => c,
      None => {
        let c = self.current_display_col();
        self.preferred_col = Some(c);
        c
      }
    };
    if let Some(next_nl) = self.text[self.cursor_pos..]
      .find('\n')
      .map(|i| i + self.cursor_pos)
    {
      let next_line_start = next_nl + 1;
      let next_line_end = self.text[next_line_start..]
        .find('\n')
        .map(|i| i + next_line_start)
        .unwrap_or(self.text.len());
      self.move_to_display_col_on_line(next_line_start, next_line_end, target_col);
    } else {
      self.cursor_pos = self.text.len();
      self.preferred_col = None;
    }
  }

  pub fn move_cursor_to_beginning_of_line(&mut self, move_up_at_bol: bool) {
    let bol = self.beginning_of_current_line();
    if move_up_at_bol && self.cursor_pos == bol {
      self.set_cursor(self.beginning_of_line(self.cursor_pos.saturating_sub(1)));
    } else {
      self.set_cursor(bol);
    }
    self.preferred_col = None;
  }

  pub fn move_cursor_to_end_of_line(&mut self, move_down_at_eol: bool) {
    let eol = self.end_of_current_line();
    if move_down_at_eol && self.cursor_pos == eol {
      let next_pos = (self.cursor_pos.saturating_add(1)).min(self.text.len());
      self.set_cursor(self.end_of_line(next_pos));
    } else {
      self.set_cursor(eol);
    }
  }

  // ===== Text elements support =====

  pub fn element_payloads(&self) -> Vec<String> {
    self
      .elements
      .iter()
      .filter_map(|e| self.text.get(e.range.clone()).map(str::to_string))
      .collect()
  }

  pub fn text_elements(&self) -> Vec<UserTextElement> {
    self
      .elements
      .iter()
      .map(|e| {
        let placeholder = self.text.get(e.range.clone()).map(str::to_string);
        UserTextElement {
          byte_range: ByteRange {
            start: e.range.start,
            end: e.range.end,
          },
          placeholder,
        }
      })
      .collect()
  }

  pub(crate) fn text_element_snapshots(&self) -> Vec<TextElementSnapshot> {
    self
      .elements
      .iter()
      .filter_map(|element| {
        self
          .text
          .get(element.range.clone())
          .map(|text| TextElementSnapshot {
            id: element.id,
            range: element.range.clone(),
            text: text.to_string(),
          })
      })
      .collect()
  }

  pub(crate) fn element_id_for_exact_range(&self, range: Range<usize>) -> Option<u64> {
    self
      .elements
      .iter()
      .find(|element| element.range == range)
      .map(|element| element.id)
  }

  /// Renames a single text element in-place, keeping it atomic.
  ///
  /// Use this when the element payload is an identifier (e.g. a placeholder) that must be
  /// updated without converting the element back into normal text.
  pub fn replace_element_payload(&mut self, old: &str, new: &str) -> bool {
    let Some(idx) = self
      .elements
      .iter()
      .position(|e| self.text.get(e.range.clone()) == Some(old))
    else {
      return false;
    };

    let range = self.elements[idx].range.clone();
    let start = range.start;
    let end = range.end;
    if start > end || end > self.text.len() {
      return false;
    }

    let removed_len = end - start;
    let inserted_len = new.len();
    let diff = inserted_len as isize - removed_len as isize;

    self.text.replace_range(range, new);
    self.wrap_cache.replace(None);
    self.preferred_col = None;

    // Update the modified element's range.
    self.elements[idx].range = start..(start + inserted_len);

    // Shift element ranges that occur after the replaced element.
    if diff != 0 {
      for (j, e) in self.elements.iter_mut().enumerate() {
        if j == idx {
          continue;
        }
        if e.range.end <= start {
          continue;
        }
        if e.range.start >= end {
          e.range.start = ((e.range.start as isize) + diff) as usize;
          e.range.end = ((e.range.end as isize) + diff) as usize;
          continue;
        }

        // Elements should not partially overlap each other; degrade gracefully by
        // snapping anything intersecting the replaced range to the new bounds.
        e.range.start = start.min(e.range.start);
        e.range.end = (start + inserted_len).max(e.range.end.saturating_add_signed(diff));
      }
    }

    // Update the cursor position to account for the edit.
    self.cursor_pos = if self.cursor_pos < start {
      self.cursor_pos
    } else if self.cursor_pos <= end {
      start + inserted_len
    } else {
      ((self.cursor_pos as isize) + diff) as usize
    };
    self.cursor_pos = self.clamp_pos_to_nearest_boundary(self.cursor_pos);

    // Keep element ordering deterministic.
    self.elements.sort_by_key(|e| e.range.start);

    true
  }

  pub fn insert_element(&mut self, text: &str) -> u64 {
    let start = self.clamp_pos_for_insertion(self.cursor_pos);
    self.insert_str_at(start, text);
    let end = start + text.len();
    let id = self.add_element(start..end);
    // Place cursor at end of inserted element
    self.set_cursor(end);
    id
  }

  /// Mark an existing text range as an atomic element without changing the text.
  ///
  /// This is used to convert already-typed tokens (like `/plan`) into elements
  /// so they render and edit atomically. Overlapping or duplicate ranges are ignored.
  pub fn add_element_range(&mut self, range: Range<usize>) -> Option<u64> {
    let start = self.clamp_pos_to_char_boundary(range.start.min(self.text.len()));
    let end = self.clamp_pos_to_char_boundary(range.end.min(self.text.len()));
    if start >= end {
      return None;
    }
    if self
      .elements
      .iter()
      .any(|e| e.range.start == start && e.range.end == end)
    {
      return None;
    }
    if self
      .elements
      .iter()
      .any(|e| start < e.range.end && end > e.range.start)
    {
      return None;
    }
    let id = self.next_element_id();
    self.elements.push(TextElement {
      id,
      range: start..end,
    });
    self.elements.sort_by_key(|e| e.range.start);
    Some(id)
  }

  pub fn remove_element_range(&mut self, range: Range<usize>) -> bool {
    let start = self.clamp_pos_to_char_boundary(range.start.min(self.text.len()));
    let end = self.clamp_pos_to_char_boundary(range.end.min(self.text.len()));
    if start >= end {
      return false;
    }
    let len_before = self.elements.len();
    self
      .elements
      .retain(|elem| elem.range.start != start || elem.range.end != end);
    len_before != self.elements.len()
  }

  fn add_element(&mut self, range: Range<usize>) -> u64 {
    let id = self.next_element_id();
    let elem = TextElement { id, range };
    self.elements.push(elem);
    self.elements.sort_by_key(|e| e.range.start);
    id
  }

  fn next_element_id(&mut self) -> u64 {
    let id = self.next_element_id;
    self.next_element_id = self.next_element_id.saturating_add(1);
    id
  }

  fn find_element_containing(&self, pos: usize) -> Option<usize> {
    self
      .elements
      .iter()
      .position(|e| pos > e.range.start && pos < e.range.end)
  }

  fn clamp_pos_to_char_boundary(&self, pos: usize) -> usize {
    let pos = pos.min(self.text.len());
    if self.text.is_char_boundary(pos) {
      return pos;
    }
    let mut prev = pos;
    while prev > 0 && !self.text.is_char_boundary(prev) {
      prev -= 1;
    }
    let mut next = pos;
    while next < self.text.len() && !self.text.is_char_boundary(next) {
      next += 1;
    }
    if pos.saturating_sub(prev) <= next.saturating_sub(pos) {
      prev
    } else {
      next
    }
  }

  fn clamp_pos_to_nearest_boundary(&self, pos: usize) -> usize {
    let pos = self.clamp_pos_to_char_boundary(pos);
    if let Some(idx) = self.find_element_containing(pos) {
      let e = &self.elements[idx];
      let dist_start = pos.saturating_sub(e.range.start);
      let dist_end = e.range.end.saturating_sub(pos);
      if dist_start <= dist_end {
        self.clamp_pos_to_char_boundary(e.range.start)
      } else {
        self.clamp_pos_to_char_boundary(e.range.end)
      }
    } else {
      pos
    }
  }

  fn clamp_pos_for_insertion(&self, pos: usize) -> usize {
    let pos = self.clamp_pos_to_char_boundary(pos);
    // Do not allow inserting into the middle of an element
    if let Some(idx) = self.find_element_containing(pos) {
      let e = &self.elements[idx];
      // Choose closest edge for insertion
      let dist_start = pos.saturating_sub(e.range.start);
      let dist_end = e.range.end.saturating_sub(pos);
      if dist_start <= dist_end {
        self.clamp_pos_to_char_boundary(e.range.start)
      } else {
        self.clamp_pos_to_char_boundary(e.range.end)
      }
    } else {
      pos
    }
  }

  fn expand_range_to_element_boundaries(&self, mut range: Range<usize>) -> Range<usize> {
    // Expand to include any intersecting elements fully
    loop {
      let mut changed = false;
      for e in &self.elements {
        if e.range.start < range.end && e.range.end > range.start {
          let new_start = range.start.min(e.range.start);
          let new_end = range.end.max(e.range.end);
          if new_start != range.start || new_end != range.end {
            range.start = new_start;
            range.end = new_end;
            changed = true;
          }
        }
      }
      if !changed {
        break;
      }
    }
    range
  }

  fn shift_elements(&mut self, at: usize, removed: usize, inserted: usize) {
    // Generic shift: for pure insert, removed = 0; for delete, inserted = 0.
    let end = at + removed;
    let diff = inserted as isize - removed as isize;
    // Remove elements fully deleted by the operation and shift the rest
    self
      .elements
      .retain(|e| !(e.range.start >= at && e.range.end <= end));
    for e in &mut self.elements {
      if e.range.end <= at {
        // before edit
      } else if e.range.start >= end {
        // after edit
        e.range.start = ((e.range.start as isize) + diff) as usize;
        e.range.end = ((e.range.end as isize) + diff) as usize;
      } else {
        // Overlap with element but not fully contained (shouldn't happen when using
        // element-aware replace, but degrade gracefully by snapping element to new bounds)
        let new_start = at.min(e.range.start);
        let new_end = at + inserted.max(e.range.end.saturating_sub(end));
        e.range.start = new_start;
        e.range.end = new_end;
      }
    }
  }

  fn update_elements_after_replace(&mut self, start: usize, end: usize, inserted_len: usize) {
    self.shift_elements(start, end.saturating_sub(start), inserted_len);
  }

  fn prev_atomic_boundary(&self, pos: usize) -> usize {
    if pos == 0 {
      return 0;
    }
    // If currently at an element end or inside, jump to start of that element.
    if let Some(idx) = self
      .elements
      .iter()
      .position(|e| pos > e.range.start && pos <= e.range.end)
    {
      return self.elements[idx].range.start;
    }
    let mut gc = unicode_segmentation::GraphemeCursor::new(pos, self.text.len(), false);
    match gc.prev_boundary(&self.text, 0) {
      Ok(Some(b)) => {
        if let Some(idx) = self.find_element_containing(b) {
          self.elements[idx].range.start
        } else {
          b
        }
      }
      Ok(None) => 0,
      Err(_) => pos.saturating_sub(1),
    }
  }

  fn next_atomic_boundary(&self, pos: usize) -> usize {
    if pos >= self.text.len() {
      return self.text.len();
    }
    // If currently at an element start or inside, jump to end of that element.
    if let Some(idx) = self
      .elements
      .iter()
      .position(|e| pos >= e.range.start && pos < e.range.end)
    {
      return self.elements[idx].range.end;
    }
    let mut gc = unicode_segmentation::GraphemeCursor::new(pos, self.text.len(), false);
    match gc.next_boundary(&self.text, 0) {
      Ok(Some(b)) => {
        if let Some(idx) = self.find_element_containing(b) {
          self.elements[idx].range.end
        } else {
          b
        }
      }
      Ok(None) => self.text.len(),
      Err(_) => pos.saturating_add(1),
    }
  }

  pub(crate) fn beginning_of_previous_word(&self) -> usize {
    let prefix = &self.text[..self.cursor_pos];
    let Some((first_non_ws_idx, ch)) = prefix
      .char_indices()
      .rev()
      .find(|&(_, ch)| !ch.is_whitespace())
    else {
      return 0;
    };
    let is_separator = is_word_separator(ch);
    let mut start = first_non_ws_idx;
    for (idx, ch) in prefix[..first_non_ws_idx].char_indices().rev() {
      if ch.is_whitespace() || is_word_separator(ch) != is_separator {
        start = idx + ch.len_utf8();
        break;
      }
      start = idx;
    }
    self.adjust_pos_out_of_elements(start, true)
  }

  pub(crate) fn end_of_next_word(&self) -> usize {
    let Some(first_non_ws) = self.text[self.cursor_pos..].find(|c: char| !c.is_whitespace()) else {
      return self.text.len();
    };
    let word_start = self.cursor_pos + first_non_ws;
    let mut iter = self.text[word_start..].char_indices();
    let Some((_, first_ch)) = iter.next() else {
      return word_start;
    };
    let is_separator = is_word_separator(first_ch);
    let mut end = self.text.len();
    for (idx, ch) in iter {
      if ch.is_whitespace() || is_word_separator(ch) != is_separator {
        end = word_start + idx;
        break;
      }
    }
    self.adjust_pos_out_of_elements(end, false)
  }

  fn adjust_pos_out_of_elements(&self, pos: usize, prefer_start: bool) -> usize {
    if let Some(idx) = self.find_element_containing(pos) {
      let e = &self.elements[idx];
      if prefer_start {
        e.range.start
      } else {
        e.range.end
      }
    } else {
      pos
    }
  }

  #[expect(clippy::unwrap_used)]
  fn wrapped_lines(&self, width: u16) -> Ref<'_, Vec<Range<usize>>> {
    // Ensure cache is ready (potentially mutably borrow, then drop)
    {
      let mut cache = self.wrap_cache.borrow_mut();
      let needs_recalc = match cache.as_ref() {
        Some(c) => c.width != width,
        None => true,
      };
      if needs_recalc {
        let lines = crate::wrapping::wrap_ranges(
          &self.text,
          Options::new(width as usize).wrap_algorithm(textwrap::WrapAlgorithm::FirstFit),
        );
        *cache = Some(WrapCache { width, lines });
      }
    }

    let cache = self.wrap_cache.borrow();
    Ref::map(cache, |c| &c.as_ref().unwrap().lines)
  }

  /// Calculate the scroll offset that should be used to satisfy the
  /// invariants given the current area size and wrapped lines.
  ///
  /// - Cursor is always on screen.
  /// - No scrolling if content fits in the area.
  fn effective_scroll(&self, area_height: u16, lines: &[Range<usize>], current_scroll: u16) -> u16 {
    let total_lines = lines.len() as u16;
    if area_height >= total_lines {
      return 0;
    }

    // Where is the cursor within wrapped lines? Prefer assigning boundary positions
    // (where pos equals the start of a wrapped line) to that later line.
    let cursor_line_idx =
      Self::wrapped_line_index_by_start(lines, self.cursor_pos).unwrap_or(0) as u16;

    let max_scroll = total_lines.saturating_sub(area_height);
    let mut scroll = current_scroll.min(max_scroll);

    // Ensure cursor is visible within [scroll, scroll + area_height)
    if cursor_line_idx < scroll {
      scroll = cursor_line_idx;
    } else if cursor_line_idx >= scroll + area_height {
      scroll = cursor_line_idx + 1 - area_height;
    }
    scroll
  }
}

impl WidgetRef for &TextArea {
  fn render_ref(&self, area: Rect, buf: &mut Buffer) {
    let lines = self.wrapped_lines(area.width);
    self.render_lines(area, buf, &lines, 0..lines.len());
  }
}

impl StatefulWidgetRef for &TextArea {
  type State = TextAreaState;

  fn render_ref(&self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
    let lines = self.wrapped_lines(area.width);
    let scroll = self.effective_scroll(area.height, &lines, state.scroll);
    state.scroll = scroll;

    let start = scroll as usize;
    let end = (scroll + area.height).min(lines.len() as u16) as usize;
    self.render_lines(area, buf, &lines, start..end);
  }
}

impl TextArea {
  pub(crate) fn render_ref_masked(
    &self,
    area: Rect,
    buf: &mut Buffer,
    state: &mut TextAreaState,
    mask_char: char,
  ) {
    let lines = self.wrapped_lines(area.width);
    let scroll = self.effective_scroll(area.height, &lines, state.scroll);
    state.scroll = scroll;

    let start = scroll as usize;
    let end = (scroll + area.height).min(lines.len() as u16) as usize;
    self.render_lines_masked(area, buf, &lines, start..end, mask_char);
  }

  fn render_lines(
    &self,
    area: Rect,
    buf: &mut Buffer,
    lines: &[Range<usize>],
    range: std::ops::Range<usize>,
  ) {
    for (row, idx) in range.enumerate() {
      let r = &lines[idx];
      let y = area.y + row as u16;
      let line_range = r.start..r.end - 1;
      // Draw base line with default style.
      buf.set_string(area.x, y, &self.text[line_range.clone()], Style::default());

      // Overlay styled segments for elements that intersect this line.
      for elem in &self.elements {
        // Compute overlap with displayed slice.
        let overlap_start = elem.range.start.max(line_range.start);
        let overlap_end = elem.range.end.min(line_range.end);
        if overlap_start >= overlap_end {
          continue;
        }
        let styled = &self.text[overlap_start..overlap_end];
        let x_off = self.text[line_range.start..overlap_start].width() as u16;
        let style = Style::default().fg(Color::Cyan);
        buf.set_string(area.x + x_off, y, styled, style);
      }
    }
  }

  fn render_lines_masked(
    &self,
    area: Rect,
    buf: &mut Buffer,
    lines: &[Range<usize>],
    range: std::ops::Range<usize>,
    mask_char: char,
  ) {
    for (row, idx) in range.enumerate() {
      let r = &lines[idx];
      let y = area.y + row as u16;
      let line_range = r.start..r.end - 1;
      let masked = self.text[line_range.clone()]
        .chars()
        .map(|_| mask_char)
        .collect::<String>();
      buf.set_string(area.x, y, &masked, Style::default());
    }
  }
}
