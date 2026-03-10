use ratatui::buffer::Buffer;
use ratatui::layout::Alignment;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::BorderType;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use ratatui::widgets::Widget;

use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_lines;

#[derive(Clone, Debug)]
pub(crate) struct ApprovalRequest {
  pub(crate) call_id: String,
  pub(crate) tool_name: String,
  pub(crate) command: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApprovalChoice {
  Allow,
  AllowAlways,
  Deny,
}

#[derive(Clone, Debug)]
pub(crate) struct ApprovalOverlay {
  request: ApprovalRequest,
  selected: ApprovalChoice,
}

impl ApprovalOverlay {
  pub(crate) fn new(request: ApprovalRequest) -> Self {
    Self {
      request,
      selected: ApprovalChoice::Allow,
    }
  }

  pub(crate) fn selected(&self) -> ApprovalChoice {
    self.selected
  }

  fn option_span(&self, choice: ApprovalChoice, label: &str) -> Span<'static> {
    if self.selected == choice {
      label.to_string().bold().add_modifier(Modifier::REVERSED)
    } else {
      label.to_string().into()
    }
  }

  fn content_lines(&self, inner_width: u16) -> Vec<Line<'static>> {
    let inner_width = inner_width.max(1);
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Wrap the tool/command so narrow terminals still display the full context.
    for line in [
      Line::from(format!("Tool: {}", self.request.tool_name)),
      Line::from(format!("Command: {}", self.request.command)),
    ] {
      let wrapped = word_wrap_lines(vec![line], RtOptions::new(inner_width as usize));
      lines.extend(wrapped);
    }

    lines.push(Line::from(""));

    lines.push(Line::from(vec![
      self.option_span(ApprovalChoice::Allow, "[Allow]"),
      Span::raw("  "),
      self.option_span(ApprovalChoice::AllowAlways, "[Allow Always]"),
      Span::raw("  "),
      self.option_span(ApprovalChoice::Deny, "[Deny]"),
    ]));

    lines
  }

  pub(crate) fn desired_height(&self, width: u16) -> u16 {
    // Border consumes 2 rows; content wraps to the remaining width.
    let inner_width = width.saturating_sub(2).max(1);
    let content_rows = self
      .content_lines(inner_width)
      .len()
      .try_into()
      .unwrap_or(u16::MAX);
    content_rows.saturating_add(2)
  }

  pub(crate) fn move_left(&mut self) {
    self.selected = match self.selected {
      ApprovalChoice::Allow => ApprovalChoice::Allow,
      ApprovalChoice::AllowAlways => ApprovalChoice::Allow,
      ApprovalChoice::Deny => ApprovalChoice::AllowAlways,
    };
  }

  pub(crate) fn move_right(&mut self) {
    self.selected = match self.selected {
      ApprovalChoice::Allow => ApprovalChoice::AllowAlways,
      ApprovalChoice::AllowAlways => ApprovalChoice::Deny,
      ApprovalChoice::Deny => ApprovalChoice::Deny,
    };
  }

  pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
    if area.width < 10 || area.height < 5 {
      return;
    }

    let w = area.width.saturating_sub(2).min(96).max(10);
    let desired_h = self.desired_height(w);
    let h = desired_h.min(area.height.saturating_sub(0).max(1));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let modal = Rect {
      x,
      y,
      width: w,
      height: h,
    };

    Clear.render(modal, buf);

    let block = Block::default()
      .title("Approval Required")
      .borders(Borders::ALL)
      .border_type(BorderType::Rounded);

    let inner_width = modal.width.saturating_sub(2).max(1);
    let lines = self.content_lines(inner_width);

    Paragraph::new(lines)
      .block(block)
      .alignment(Alignment::Left)
      .wrap(Wrap { trim: true })
      .render(modal, buf);
  }
}
