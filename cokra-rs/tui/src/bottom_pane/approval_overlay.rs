//! 1:1 codex ApprovalOverlay: a BottomPaneView backed by ListSelectionView.
//!
//! When the agent requests approval, this view replaces the composer in the
//! bottom pane, showing the tool/command and a list of selectable options.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

use super::bottom_pane_view::BottomPaneView;
use super::list_selection_view::ListSelectionView;
use super::list_selection_view::SelectionItem;
use super::list_selection_view::SelectionViewParams;
use crate::app_event_sender::AppEventSender;
use crate::render::renderable::ColumnRenderable;
use crate::render::renderable::Renderable;
use crate::tui::InlineViewportSizing;

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

/// 1:1 codex ApprovalOverlay: modal view shown in the bottom pane view_stack.
pub(crate) struct ApprovalOverlay {
  request: ApprovalRequest,
  list: ListSelectionView,
  options: Vec<ApprovalChoice>,
  done: bool,
  selected_choice: Option<ApprovalChoice>,
}

impl ApprovalOverlay {
  pub(crate) fn new(request: ApprovalRequest, app_event_tx: AppEventSender) -> Self {
    let options = vec![
      ApprovalChoice::Allow,
      ApprovalChoice::AllowAlways,
      ApprovalChoice::Deny,
    ];

    let header = build_header(&request);

    let items = vec![
      SelectionItem {
        name: "Yes, proceed".to_string(),
        dismiss_on_select: false,
        ..Default::default()
      },
      SelectionItem {
        name: "Yes, and don't ask again for this tool in this session".to_string(),
        dismiss_on_select: false,
        ..Default::default()
      },
      SelectionItem {
        name: "No, and tell the agent what to do differently".to_string(),
        dismiss_on_select: false,
        ..Default::default()
      },
    ];

    let header = Box::new(ColumnRenderable::with([
      Line::from(
        "Would you like to allow the following tool call?".bold(),
      )
      .into(),
      Line::from("").into(),
      header,
    ]));

    let params = SelectionViewParams {
      footer_hint: Some(Line::from(vec![
        "Press ".into(),
        "Enter".bold(),
        " to confirm, ".into(),
        "y".bold(),
        " allow, ".into(),
        "a".bold(),
        " always, ".into(),
        "n".bold(),
        "/".into(),
        "Esc".bold(),
        " deny".into(),
      ])),
      items,
      header,
      ..Default::default()
    };

    let list = ListSelectionView::new(params, app_event_tx);

    Self {
      request,
      list,
      options,
      done: false,
      selected_choice: None,
    }
  }

  pub(crate) fn request(&self) -> &ApprovalRequest {
    &self.request
  }

  pub(crate) fn take_choice(&mut self) -> Option<ApprovalChoice> {
    self.selected_choice.take()
  }

  fn apply_selection(&mut self, idx: usize) {
    if self.done {
      return;
    }
    if let Some(choice) = self.options.get(idx) {
      self.selected_choice = Some(*choice);
      self.done = true;
    }
  }
}

impl BottomPaneView for ApprovalOverlay {
  fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
    self
  }

  fn inline_viewport_sizing(&self) -> InlineViewportSizing {
    InlineViewportSizing::ExpandForOverlay
  }

  fn handle_key_event(&mut self, key_event: KeyEvent) {
    if key_event.kind != KeyEventKind::Press {
      return;
    }
    // Shortcut keys: y=Allow, a=AllowAlways, n=Deny
    match key_event.code {
      KeyCode::Char('y') => {
        self.apply_selection(0);
        return;
      }
      KeyCode::Char('a') => {
        self.apply_selection(1);
        return;
      }
      KeyCode::Char('n') => {
        self.apply_selection(2);
        return;
      }
      _ => {}
    }
    self.list.handle_key_event(key_event);
    if let Some(idx) = self.list.take_last_selected_index() {
      self.apply_selection(idx);
    }
  }

  fn is_complete(&self) -> bool {
    self.done
  }

  fn on_cancel(&mut self) -> bool {
    // Esc = Deny
    self.selected_choice = Some(ApprovalChoice::Deny);
    self.done = true;
    true
  }
}

impl Renderable for ApprovalOverlay {
  fn desired_height(&self, width: u16) -> u16 {
    self.list.desired_height(width)
  }

  fn render(&self, area: Rect, buf: &mut Buffer) {
    self.list.render(area, buf);
  }

  fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
    self.list.cursor_pos(area)
  }
}

fn build_header(request: &ApprovalRequest) -> Box<dyn Renderable> {
  let lines = vec![
    Line::from(vec![
      "Tool: ".into(),
      request.tool_name.clone().bold(),
    ]),
    Line::from(""),
    Line::from(request.command.clone()),
  ];
  Box::new(Paragraph::new(lines).wrap(Wrap { trim: false }))
}
