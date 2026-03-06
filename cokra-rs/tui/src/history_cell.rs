use std::any::Any;

use ratatui::prelude::Buffer;
use ratatui::prelude::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

use cokra_protocol::TextElement;

use crate::exec_cell::ExecCall;
use crate::exec_cell::ExecCell;
use crate::exec_cell::model::CommandOutput;
use crate::exec_cell::new_active_exec_command;
use crate::render::renderable::Renderable;
use crate::style::user_message_style;
use crate::welcome::WelcomeWidget;
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
  pub(crate) message: String,
  pub(crate) text_elements: Vec<TextElement>,
  pub(crate) remote_image_urls: Vec<String>,
}

impl UserHistoryCell {
  pub(crate) fn from_text(text: String) -> Self {
    Self {
      message: text,
      text_elements: Vec::new(),
      remote_image_urls: Vec::new(),
    }
  }

  pub(crate) fn new(
    message: String,
    text_elements: Vec<TextElement>,
    remote_image_urls: Vec<String>,
  ) -> Self {
    Self {
      message,
      text_elements,
      remote_image_urls,
    }
  }
}

/// 1:1 codex: Build logical lines for a user message with styled text elements.
///
/// Preserves explicit newlines while interleaving element spans; skips
/// malformed byte ranges instead of panicking during history rendering.
fn build_user_message_lines_with_elements(
  message: &str,
  elements: &[TextElement],
  style: Style,
  element_style: Style,
) -> Vec<Line<'static>> {
  let mut elements = elements.to_vec();
  elements.sort_by_key(|e| e.byte_range.start);
  let mut offset = 0usize;
  let mut raw_lines: Vec<Line<'static>> = Vec::new();
  for line_text in message.split('\n') {
    let line_start = offset;
    let line_end = line_start + line_text.len();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut cursor = line_start;
    for elem in &elements {
      let start = elem.byte_range.start.max(line_start);
      let end = elem.byte_range.end.min(line_end);
      if start >= end {
        continue;
      }
      let rel_start = start - line_start;
      let rel_end = end - line_start;
      if !line_text.is_char_boundary(rel_start) || !line_text.is_char_boundary(rel_end) {
        continue;
      }
      let rel_cursor = cursor - line_start;
      if cursor < start
        && line_text.is_char_boundary(rel_cursor)
        && let Some(segment) = line_text.get(rel_cursor..rel_start)
      {
        spans.push(Span::from(segment.to_string()));
      }
      if let Some(segment) = line_text.get(rel_start..rel_end) {
        spans.push(Span::styled(segment.to_string(), element_style));
        cursor = end;
      }
    }
    let rel_cursor = cursor - line_start;
    if cursor < line_end
      && line_text.is_char_boundary(rel_cursor)
      && let Some(segment) = line_text.get(rel_cursor..)
    {
      spans.push(Span::from(segment.to_string()));
    }
    let line = if spans.is_empty() {
      Line::from(line_text.to_string()).style(style)
    } else {
      Line::from(spans).style(style)
    };
    raw_lines.push(line);
    offset = line_end + 1;
  }
  raw_lines
}

fn trim_trailing_blank_lines(mut lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
  while lines
    .last()
    .is_some_and(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
  {
    lines.pop();
  }
  lines
}

/// 1:1 codex: prefix_lines adds a gutter prefix to each line.
fn prefix_lines(
  lines: Vec<Line<'static>>,
  first_prefix: Span<'static>,
  continuation_prefix: Span<'static>,
) -> Vec<Line<'static>> {
  lines
    .into_iter()
    .enumerate()
    .map(|(i, mut line)| {
      let prefix = if i == 0 {
        first_prefix.clone()
      } else {
        continuation_prefix.clone()
      };
      line.spans.insert(0, prefix);
      line
    })
    .collect()
}

impl HistoryCell for UserHistoryCell {
  fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
    let wrap_width = width.saturating_sub(3).max(1); // "› " prefix = 2 cols + 1 margin
    let style = user_message_style();
    let element_style = style.fg(Color::Cyan);

    let wrapped_remote_images = if self.remote_image_urls.is_empty() {
      None
    } else {
      Some(word_wrap_lines(
        self
          .remote_image_urls
          .iter()
          .enumerate()
          .map(|(idx, _url)| Line::from(format!("[image {}]", idx + 1)).style(element_style)),
        RtOptions::new(wrap_width as usize),
      ))
    };

    let wrapped_message = if self.message.is_empty() && self.text_elements.is_empty() {
      None
    } else if self.text_elements.is_empty() {
      let msg = self.message.trim_end_matches(['\r', '\n']);
      let wrapped = word_wrap_lines(
        msg
          .split('\n')
          .map(|line| Line::from(line.to_string()).style(style)),
        RtOptions::new(wrap_width as usize),
      );
      let wrapped = trim_trailing_blank_lines(wrapped);
      (!wrapped.is_empty()).then_some(wrapped)
    } else {
      let raw_lines = build_user_message_lines_with_elements(
        &self.message,
        &self.text_elements,
        style,
        element_style,
      );
      let wrapped = word_wrap_lines(raw_lines, RtOptions::new(wrap_width as usize));
      let wrapped = trim_trailing_blank_lines(wrapped);
      (!wrapped.is_empty()).then_some(wrapped)
    };

    if wrapped_remote_images.is_none() && wrapped_message.is_none() {
      return Vec::new();
    }

    let mut lines: Vec<Line<'static>> = Vec::new();

    if let Some(imgs) = wrapped_remote_images {
      lines.extend(prefix_lines(imgs, "  ".into(), "  ".into()));
      if wrapped_message.is_some() {
        lines.push(Line::from("").style(style));
      }
    }

    if let Some(msg) = wrapped_message {
      lines.extend(prefix_lines(msg, "› ".bold().dim(), "  ".into()));
    }

    lines
  }

  fn desired_height(&self, width: u16) -> u16 {
    self
      .display_lines(width)
      .len()
      .try_into()
      .unwrap_or(u16::MAX)
  }
}

#[derive(Debug)]
pub(crate) struct CompositeHistoryCell {
  parts: Vec<Box<dyn HistoryCell>>,
}

impl CompositeHistoryCell {
  pub(crate) fn new(parts: Vec<Box<dyn HistoryCell>>) -> Self {
    Self { parts }
  }
}

impl HistoryCell for CompositeHistoryCell {
  fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut first = true;
    for part in &self.parts {
      let mut lines = part.display_lines(width);
      if lines.is_empty() {
        continue;
      }
      if !first {
        out.push(Line::from(""));
      }
      out.append(&mut lines);
      first = false;
    }
    out
  }
}

#[derive(Debug)]
pub(crate) struct SessionInfoCell(CompositeHistoryCell);

impl HistoryCell for SessionInfoCell {
  fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
    self.0.display_lines(width)
  }

  fn desired_height(&self, width: u16) -> u16 {
    self.0.desired_height(width)
  }

  fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
    self.0.transcript_lines(width)
  }
}

#[derive(Debug)]
pub(crate) struct SessionHeaderHistoryCell {
  model: String,
  approval_policy: String,
  sandbox_mode: String,
  cwd: Option<String>,
}

impl SessionHeaderHistoryCell {
  pub(crate) fn new(
    model: String,
    approval_policy: String,
    sandbox_mode: String,
    cwd: Option<String>,
  ) -> Self {
    Self {
      model,
      approval_policy,
      sandbox_mode,
      cwd,
    }
  }
}

impl HistoryCell for SessionHeaderHistoryCell {
  fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
      "┌─ ".dim(),
      "cokra".bold(),
      " ─ ".dim(),
      self.model.clone().into(),
    ]));
    lines.push(Line::from(vec![
      "│  ".dim(),
      "sandbox: ".dim(),
      self.sandbox_mode.clone().into(),
      " │ approval: ".dim(),
      self.approval_policy.clone().into(),
    ]));

    if let Some(cwd) = &self.cwd {
      lines.push(Line::from(vec![
        "│  ".dim(),
        "cwd: ".dim(),
        cwd.clone().into(),
      ]));
    }

    lines.push(Line::from("└──".dim()));
    lines
  }
}

pub(crate) fn new_session_info(
  model: String,
  approval_policy: String,
  sandbox_mode: String,
  cwd: Option<String>,
  is_first_session: bool,
) -> SessionInfoCell {
  let mut parts: Vec<Box<dyn HistoryCell>> = Vec::new();

  if is_first_session {
    parts.push(WelcomeWidget::into_history_cell());
  }

  parts.push(Box::new(SessionHeaderHistoryCell::new(
    model,
    approval_policy,
    sandbox_mode,
    cwd,
  )));

  if is_first_session {
    parts.push(Box::new(PlainHistoryCell::new(vec![
      Line::from("  To get started, describe a task or try one of these commands:").dim(),
      Line::from(""),
      Line::from(vec![
        "  ".into(),
        "/help".into(),
        " - show available commands".dim(),
      ]),
      Line::from(vec![
        "  ".into(),
        "/model".into(),
        " - choose model and reasoning effort".dim(),
      ]),
      Line::from(vec![
        "  ".into(),
        "/status".into(),
        " - show current session configuration".dim(),
      ]),
    ])));
  }

  SessionInfoCell(CompositeHistoryCell::new(parts))
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

  pub(crate) fn append_lines(&mut self, new_lines: Vec<Line<'static>>) {
    self.lines.extend(new_lines);
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
    tool_name: String,
    command: String,
    cwd: std::path::PathBuf,
    exit_code: i32,
    output: String,
    duration: std::time::Duration,
    animations_enabled: bool,
  ) -> Self {
    let mut cell = new_active_exec_command(command_id, tool_name, command, cwd, animations_enabled);
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
  fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
    // 1:1 codex FinalMessageSeparator: visual divider with optional token summary.
    let label = if self.input_tokens > 0 || self.output_tokens > 0 {
      format!("─ {} in / {} out ─", self.input_tokens, self.output_tokens)
    } else {
      String::new()
    };

    if label.is_empty() {
      return vec![Line::from("─".repeat(width as usize)).dim()];
    }

    let label_width = label.chars().count();
    let remaining = (width as usize).saturating_sub(label_width);
    vec![Line::from(vec![label.dim(), "─".repeat(remaining).dim()])]
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

#[cfg(test)]
mod tests {
  use super::*;

  fn lines_to_string(lines: &[Line<'static>]) -> String {
    lines
      .iter()
      .map(|line| {
        line
          .spans
          .iter()
          .map(|span| span.content.as_ref())
          .collect::<String>()
      })
      .collect::<Vec<_>>()
      .join("\n")
  }

  #[test]
  fn first_session_info_includes_welcome_header_and_help() {
    let cell = new_session_info(
      "openrouter/anthropic/claude-haiku-4.5".to_string(),
      "Ask".to_string(),
      "Permissive".to_string(),
      None,
      true,
    );

    let rendered = lines_to_string(&cell.display_lines(80));
    assert!(rendered.contains("Welcome to Cokra, AI Agent Team CLI Environment"));
    assert!(rendered.contains("┌─ cokra ─ openrouter/anthropic/claude-haiku-4.5"));
    assert!(rendered.contains("/help - show available commands"));
  }

  #[test]
  fn non_first_session_info_only_renders_header() {
    let cell = new_session_info(
      "openrouter/anthropic/claude-haiku-4.5".to_string(),
      "Ask".to_string(),
      "Permissive".to_string(),
      Some("/tmp/project".to_string()),
      false,
    );

    let rendered = lines_to_string(&cell.display_lines(80));
    assert!(!rendered.contains("Welcome to Cokra"));
    assert!(!rendered.contains("/help - show available commands"));
    assert!(rendered.contains("cwd: /tmp/project"));
  }
}
