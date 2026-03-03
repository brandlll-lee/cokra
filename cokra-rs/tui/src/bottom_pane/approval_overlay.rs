use ratatui::buffer::Buffer;
use ratatui::layout::Alignment;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Stylize;
use ratatui::widgets::Block;
use ratatui::widgets::BorderType;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

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
    if area.width < 10 || area.height < 6 {
      return;
    }

    let w = area.width.saturating_mul(3) / 4;
    let h = area.height.saturating_mul(3) / 5;
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

    let selected = |choice: ApprovalChoice, label: &str| -> ratatui::text::Span<'static> {
      if self.selected == choice {
        label.to_string().bold().add_modifier(Modifier::REVERSED)
      } else {
        label.to_string().into()
      }
    };

    let content = vec![
      format!("Tool: {}", self.request.tool_name),
      format!("Command: {}", self.request.command),
      String::new(),
      format!(
        "{}  {}  {}",
        selected(ApprovalChoice::Allow, "[Allow]"),
        selected(ApprovalChoice::AllowAlways, "[Allow Always]"),
        selected(ApprovalChoice::Deny, "[Deny]")
      ),
    ]
    .join("\n");

    Paragraph::new(content)
      .block(block)
      .alignment(Alignment::Left)
      .render(modal, buf);
  }
}
