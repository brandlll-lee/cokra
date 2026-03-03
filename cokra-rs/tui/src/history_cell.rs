use std::any::Any;

use ratatui::prelude::Buffer;
use ratatui::prelude::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

use crate::exec_cell::ExecCall;
use crate::exec_cell::ExecCell;
use crate::exec_cell::model::CommandOutput;
use crate::exec_cell::new_active_exec_command;
use crate::render::renderable::Renderable;
use crate::style::user_message_style;
use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_lines;

pub(crate) trait HistoryCell: std::fmt::Debug + Send + Sync + Any {
  fn display_lines(&self, width: u16) -> Vec<Line<'static>>;

  fn desired_height(&self, width: u16) -> u16 {
    Paragraph::new(Text::from(self.display_lines(width)))
      .wrap(Wrap { trim: false })
      .line_count(width)
      .try_into()
      .unwrap_or(0)
  }

  fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
    self.display_lines(width)
  }

  fn desired_transcript_height(&self, width: u16) -> u16 {
    Paragraph::new(Text::from(self.transcript_lines(width)))
      .wrap(Wrap { trim: false })
      .line_count(width)
      .try_into()
      .unwrap_or(0)
  }

  fn is_stream_continuation(&self) -> bool {
    false
  }

  fn transcript_animation_tick(&self) -> Option<u64> {
    None
  }
}

impl Renderable for Box<dyn HistoryCell> {
  fn render(&self, area: Rect, buf: &mut Buffer) {
    let lines = self.display_lines(area.width);
    let y = if area.height == 0 {
      0
    } else {
      let overflow = lines.len().saturating_sub(usize::from(area.height));
      u16::try_from(overflow).unwrap_or(u16::MAX)
    };
    Paragraph::new(Text::from(lines))
      .scroll((y, 0))
      .render(area, buf);
  }

  fn desired_height(&self, width: u16) -> u16 {
    HistoryCell::desired_height(self.as_ref(), width)
  }
}

impl dyn HistoryCell {
  #[allow(dead_code)]
  pub(crate) fn as_any(&self) -> &dyn Any {
    self
  }

  #[allow(dead_code)]
  pub(crate) fn as_any_mut(&mut self) -> &mut dyn Any {
    self
  }
}

#[derive(Debug)]
pub(crate) struct PlainHistoryCell {
  pub(crate) lines: Vec<Line<'static>>,
}

impl PlainHistoryCell {
  pub(crate) fn new(lines: Vec<Line<'static>>) -> Self {
    Self { lines }
  }
}

impl HistoryCell for PlainHistoryCell {
  fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
    self.lines.clone()
  }
}

#[derive(Debug)]
pub(crate) struct UserHistoryCell {
  pub(crate) lines: Vec<Line<'static>>,
  pub(crate) raw_text: String,
}

impl UserHistoryCell {
  pub(crate) fn from_text(text: String) -> Self {
    Self {
      lines: vec![Line::from(text.clone())],
      raw_text: text,
    }
  }
}

impl HistoryCell for UserHistoryCell {
  fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
    word_wrap_lines(
      &self.lines,
      RtOptions::new(width.max(1) as usize)
        .initial_indent(vec!["› ".bold().style(user_message_style())].into())
        .subsequent_indent("  ".into()),
    )
  }
}

#[derive(Debug)]
pub(crate) struct SessionConfiguredCell {
  pub(crate) model: String,
  pub(crate) approval_policy: String,
  pub(crate) sandbox_mode: String,
}

impl HistoryCell for SessionConfiguredCell {
  fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
    vec![
      Line::from(vec![
        "┌─ ".dim(),
        format!(
          "model: {} | sandbox: {} | approval: {}",
          self.model, self.sandbox_mode, self.approval_policy
        )
        .into(),
      ]),
      Line::from(vec!["└─ ".dim(), "session configured".dim()]),
    ]
  }
}

#[derive(Debug)]
pub(crate) struct AgentMessageCell {
  pub(crate) lines: Vec<Line<'static>>,
  pub(crate) is_first_line: bool,
}

impl AgentMessageCell {
  pub(crate) fn new(lines: Vec<Line<'static>>, is_first_line: bool) -> Self {
    Self {
      lines,
      is_first_line,
    }
  }
}

impl HistoryCell for AgentMessageCell {
  fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
    word_wrap_lines(
      &self.lines,
      RtOptions::new(width.max(1) as usize)
        .initial_indent(if self.is_first_line {
          "• ".dim().into()
        } else {
          "  ".into()
        })
        .subsequent_indent("  ".into()),
    )
  }

  fn is_stream_continuation(&self) -> bool {
    !self.is_first_line
  }
}

#[derive(Debug)]
pub(crate) struct ExecHistoryCell {
  pub(crate) exec_cell: ExecCell,
}

impl ExecHistoryCell {
  pub(crate) fn from_completed_call(
    command_id: String,
    command: String,
    cwd: std::path::PathBuf,
    exit_code: i32,
    output: String,
    duration: std::time::Duration,
    animations_enabled: bool,
  ) -> Self {
    let mut cell = new_active_exec_command(command_id, command, cwd, animations_enabled);
    let last_id = cell
      .calls
      .last()
      .map(|c| c.command_id.clone())
      .unwrap_or_default();
    cell.complete_call(&last_id, CommandOutput { exit_code, output }, duration);
    Self { exec_cell: cell }
  }

  pub(crate) fn from_exec_call(call: ExecCall, animations_enabled: bool) -> Self {
    Self {
      exec_cell: ExecCell::new(call, animations_enabled),
    }
  }
}

impl HistoryCell for ExecHistoryCell {
  fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
    self.exec_cell.display_lines(width)
  }

  fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
    self.exec_cell.transcript_lines(width)
  }
}

#[derive(Debug)]
pub(crate) struct ApprovalRequestedHistoryCell {
  pub(crate) command: String,
}

impl HistoryCell for ApprovalRequestedHistoryCell {
  fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
    word_wrap_lines(
      &[Line::from(format!("Awaiting approval: {}", self.command))],
      RtOptions::new(width.max(1) as usize)
        .initial_indent("• ".dim().into())
        .subsequent_indent("  ".into()),
    )
  }
}

#[derive(Debug)]
pub(crate) struct TurnCompleteHistoryCell {
  pub(crate) input_tokens: i64,
  pub(crate) output_tokens: i64,
}

impl HistoryCell for TurnCompleteHistoryCell {
  fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
    vec![Line::from(format!(
      " ─── turn complete: {} in / {} out",
      self.input_tokens, self.output_tokens
    ))]
  }
}

#[derive(Debug)]
pub(crate) struct ProposedPlanStreamCell {
  lines: Vec<Line<'static>>,
  is_stream_continuation: bool,
}

impl HistoryCell for ProposedPlanStreamCell {
  fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
    self.lines.clone()
  }

  fn is_stream_continuation(&self) -> bool {
    self.is_stream_continuation
  }
}

pub(crate) fn new_proposed_plan_stream(
  lines: Vec<Line<'static>>,
  is_stream_continuation: bool,
) -> ProposedPlanStreamCell {
  ProposedPlanStreamCell {
    lines,
    is_stream_continuation,
  }
}
