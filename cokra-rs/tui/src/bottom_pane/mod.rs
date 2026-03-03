use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;

use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

pub(crate) mod approval_overlay;
pub(crate) mod chat_composer;
pub(crate) mod footer;
pub(crate) mod textarea;

use approval_overlay::ApprovalChoice;
use approval_overlay::ApprovalOverlay;
use chat_composer::ChatComposer;
use chat_composer::ComposerAction;
use chat_composer::ComposerSubmission;
use footer::FooterMode;
use footer::FooterProps;

#[derive(Debug)]
pub(crate) enum BottomPaneAction {
  None,
  Submit(ComposerSubmission),
  Interrupt,
  RequestQuit,
  ApprovalDecision(ApprovalChoice),
}

#[derive(Debug)]
pub(crate) struct BottomPane {
  composer: ChatComposer,
  approval_overlay: Option<ApprovalOverlay>,
}

impl BottomPane {
  pub(crate) fn new() -> Self {
    Self {
      composer: ChatComposer::new(),
      approval_overlay: None,
    }
  }

  pub(crate) fn set_task_running(&mut self, running: bool) {
    self.composer.set_task_running(running);
  }

  pub(crate) fn open_approval(&mut self, overlay: ApprovalOverlay) {
    self.approval_overlay = Some(overlay);
  }

  pub(crate) fn desired_height(&self, _width: u16) -> u16 {
    4
  }

  pub(crate) fn handle_key(&mut self, key: KeyEvent) -> BottomPaneAction {
    if let Some(overlay) = self.approval_overlay.as_mut() {
      match key.code {
        KeyCode::Left => overlay.move_left(),
        KeyCode::Right | KeyCode::Tab => overlay.move_right(),
        KeyCode::Enter => {
          let selected = overlay.selected();
          self.approval_overlay = None;
          return BottomPaneAction::ApprovalDecision(selected);
        }
        KeyCode::Esc => {
          self.approval_overlay = None;
          return BottomPaneAction::ApprovalDecision(ApprovalChoice::Deny);
        }
        _ => {}
      }
      return BottomPaneAction::None;
    }

    match self.composer.handle_key_event(key) {
      ComposerAction::None | ComposerAction::Queue => BottomPaneAction::None,
      ComposerAction::Interrupt => BottomPaneAction::Interrupt,
      ComposerAction::RequestQuit => BottomPaneAction::RequestQuit,
      ComposerAction::Submit => self
        .composer
        .prepare_submission()
        .map(BottomPaneAction::Submit)
        .unwrap_or(BottomPaneAction::None),
    }
  }

  pub(crate) fn handle_paste(&mut self, text: String) {
    self.composer.handle_paste(text);
  }

  pub(crate) fn render(&self, area: Rect, buf: &mut Buffer, task_running: bool) {
    if area.height == 0 {
      return;
    }

    let chunks = Layout::vertical([Constraint::Min(2), Constraint::Length(1)]).split(area);

    let input = self
      .composer
      .render_lines(chunks[0].width.saturating_sub(2), true);
    let input_block = Block::default().title("Input").borders(Borders::ALL);
    Paragraph::new(input)
      .block(input_block)
      .render(chunks[0], buf);

    let footer_props = FooterProps {
      mode: if task_running {
        FooterMode::TaskRunning
      } else {
        FooterMode::Default
      },
      is_task_running: task_running,
      ..FooterProps::default()
    };
    footer::render_footer_from_props(&footer_props, chunks[1], buf);

    if let Some(overlay) = &self.approval_overlay {
      overlay.render(area, buf);
    }
  }
}
