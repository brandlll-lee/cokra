use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::render::renderable::Renderable;
use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_lines;

pub(crate) struct QueuedUserMessages {
  pub(crate) messages: Vec<String>,
}

impl QueuedUserMessages {
  pub(crate) fn new() -> Self {
    Self {
      messages: Vec::new(),
    }
  }

  fn as_renderable(&self, width: u16) -> Box<dyn Renderable> {
    if self.messages.is_empty() || width < 4 {
      return Box::new(());
    }

    let mut lines = Vec::new();
    for message in &self.messages {
      let wrapped = word_wrap_lines(
        message.lines().map(|line| line.dim().italic()),
        RtOptions::new(width as usize)
          .initial_indent(Line::from("  ↳ ".dim()))
          .subsequent_indent(Line::from("    ")),
      );
      let total = wrapped.len();
      for line in wrapped.into_iter().take(3) {
        lines.push(line);
      }
      if total > 3 {
        lines.push(Line::from("    …".dim().italic()));
      }
    }

    Paragraph::new(lines).into()
  }
}

impl Renderable for QueuedUserMessages {
  fn render(&self, area: Rect, buf: &mut Buffer) {
    if area.is_empty() {
      return;
    }
    self.as_renderable(area.width).render(area, buf);
  }

  fn desired_height(&self, width: u16) -> u16 {
    self.as_renderable(width).desired_height(width)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn desired_height_empty_is_zero() {
    let queue = QueuedUserMessages::new();
    assert_eq!(queue.desired_height(40), 0);
  }

  #[test]
  fn desired_height_with_message_is_positive() {
    let mut queue = QueuedUserMessages::new();
    queue.messages.push("Queued follow-up question".to_string());
    assert!(queue.desired_height(40) > 0);
  }
}
