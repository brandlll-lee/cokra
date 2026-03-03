use ratatui::style::Stylize;
use ratatui::text::Line;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CursorPosition {
  pub(crate) row: usize,
  pub(crate) col: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TextElement {
  pub(crate) start: usize,
  pub(crate) end: usize,
  pub(crate) kind: TextElementKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TextElementKind {
  Placeholder(String),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TextArea {
  lines: Vec<String>,
  cursor: CursorPosition,
  text_elements: Vec<TextElement>,
}

impl TextArea {
  pub(crate) fn new() -> Self {
    Self {
      lines: vec![String::new()],
      cursor: CursorPosition::default(),
      text_elements: Vec::new(),
    }
  }

  pub(crate) fn clear(&mut self) {
    self.lines.clear();
    self.lines.push(String::new());
    self.cursor = CursorPosition::default();
    self.text_elements.clear();
  }

  pub(crate) fn text_content(&self) -> String {
    self.lines.join("\n")
  }

  pub(crate) fn text_elements(&self) -> &[TextElement] {
    &self.text_elements
  }

  pub(crate) fn insert_char(&mut self, ch: char) {
    let line = &mut self.lines[self.cursor.row];
    if self.cursor.col >= line.len() {
      line.push(ch);
      self.cursor.col = line.len();
      return;
    }
    line.insert(self.cursor.col, ch);
    self.cursor.col += ch.len_utf8();
  }

  pub(crate) fn insert_str(&mut self, s: &str) {
    for ch in s.chars() {
      if ch == '\n' {
        self.insert_newline();
      } else {
        self.insert_char(ch);
      }
    }
  }

  pub(crate) fn insert_newline(&mut self) {
    let rest = {
      let line = &mut self.lines[self.cursor.row];
      if self.cursor.col >= line.len() {
        String::new()
      } else {
        line.split_off(self.cursor.col)
      }
    };
    self.cursor.row += 1;
    self.cursor.col = 0;
    self.lines.insert(self.cursor.row, rest);
  }

  pub(crate) fn delete_char_backward(&mut self) {
    if self.cursor.col > 0 {
      let line = &mut self.lines[self.cursor.row];
      let remove_at = self.cursor.col - 1;
      line.remove(remove_at);
      self.cursor.col = remove_at;
      return;
    }

    if self.cursor.row == 0 {
      return;
    }

    let current = self.lines.remove(self.cursor.row);
    self.cursor.row -= 1;
    let prev = &mut self.lines[self.cursor.row];
    let old_len = prev.len();
    prev.push_str(&current);
    self.cursor.col = old_len;
  }

  pub(crate) fn move_cursor_left(&mut self) {
    if self.cursor.col > 0 {
      self.cursor.col -= 1;
      return;
    }
    if self.cursor.row > 0 {
      self.cursor.row -= 1;
      self.cursor.col = self.lines[self.cursor.row].len();
    }
  }

  pub(crate) fn move_cursor_right(&mut self) {
    let line_len = self.lines[self.cursor.row].len();
    if self.cursor.col < line_len {
      self.cursor.col += 1;
      return;
    }
    if self.cursor.row + 1 < self.lines.len() {
      self.cursor.row += 1;
      self.cursor.col = 0;
    }
  }

  pub(crate) fn move_cursor_up(&mut self) {
    if self.cursor.row == 0 {
      return;
    }
    self.cursor.row -= 1;
    self.cursor.col = self.cursor.col.min(self.lines[self.cursor.row].len());
  }

  pub(crate) fn move_cursor_down(&mut self) {
    if self.cursor.row + 1 >= self.lines.len() {
      return;
    }
    self.cursor.row += 1;
    self.cursor.col = self.cursor.col.min(self.lines[self.cursor.row].len());
  }

  pub(crate) fn move_cursor_home(&mut self) {
    self.cursor.col = 0;
  }

  pub(crate) fn move_cursor_end(&mut self) {
    self.cursor.col = self.lines[self.cursor.row].len();
  }

  pub(crate) fn render_lines(&self, _width: u16, show_cursor: bool) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for (row, line) in self.lines.iter().enumerate() {
      if !show_cursor || row != self.cursor.row {
        out.push(Line::from(line.clone()));
        continue;
      }

      if self.cursor.col >= line.len() {
        out.push(Line::from(vec![line.clone().into(), " ".reversed()]));
        continue;
      }

      let mut chars: Vec<char> = line.chars().collect();
      let idx = self.cursor.col.min(chars.len().saturating_sub(1));
      let cursor_ch = chars.remove(idx);
      let left: String = chars.iter().take(idx).collect();
      let right: String = chars.iter().skip(idx).collect();
      out.push(Line::from(vec![
        left.into(),
        cursor_ch.to_string().reversed(),
        right.into(),
      ]));
    }
    out
  }

  pub(crate) fn cursor_render_position(&self, _width: u16) -> (u16, u16) {
    (
      u16::try_from(self.cursor.col).unwrap_or(u16::MAX),
      u16::try_from(self.cursor.row).unwrap_or(u16::MAX),
    )
  }
}
