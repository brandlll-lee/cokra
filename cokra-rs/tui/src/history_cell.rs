use std::any::Any;
use std::collections::HashMap;

use ratatui::prelude::Buffer;
use ratatui::prelude::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

use cokra_protocol::RequestUserInputAnswer;
use cokra_protocol::RequestUserInputQuestion;
use cokra_protocol::TextElement;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

use crate::exec_cell::ExecCall;
use crate::exec_cell::ExecCell;
use crate::exec_cell::model::CommandOutput;
use crate::exec_cell::new_active_exec_command;
use crate::render::renderable::Renderable;
use crate::status_indicator_widget::fmt_elapsed_compact;
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
pub(crate) struct RequestUserInputResultCell {
  pub(crate) questions: Vec<RequestUserInputQuestion>,
  pub(crate) answers: HashMap<String, RequestUserInputAnswer>,
  pub(crate) interrupted: bool,
}

impl HistoryCell for RequestUserInputResultCell {
  fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
    let width = width.max(1) as usize;
    let total = self.questions.len();
    let answered = self
      .questions
      .iter()
      .filter(|question| {
        self
          .answers
          .get(&question.id)
          .is_some_and(|answer| !answer.answers.is_empty())
      })
      .count();
    let unanswered = total.saturating_sub(answered);

    let mut header = vec!["●".dim(), " ".into(), "Questions".bold()];
    header.push(format!(" {answered}/{total} answered").dim());
    if self.interrupted {
      header.push(" (submitted early)".cyan());
    }

    let mut lines: Vec<Line<'static>> = vec![header.into()];

    for question in &self.questions {
      let answer = self.answers.get(&question.id);
      let answer_missing = match answer {
        Some(answer) => answer.answers.is_empty(),
        None => true,
      };

      let mut question_lines = wrap_with_prefix(
        &question.question,
        width,
        "  ● ".into(),
        "    ".into(),
        Style::default(),
      );
      if answer_missing && let Some(last) = question_lines.last_mut() {
        last.spans.push(" (unanswered)".dim());
      }
      lines.extend(question_lines);

      let Some(answer) = answer.filter(|answer| !answer.answers.is_empty()) else {
        continue;
      };

      if question.is_secret {
        lines.extend(wrap_with_prefix(
          "••••••",
          width,
          "    answer: ".dim(),
          "            ".dim(),
          Style::default().fg(Color::Cyan),
        ));
        continue;
      }

      let (options, note) = split_request_user_input_answer(answer);
      for option in options {
        lines.extend(wrap_with_prefix(
          &option,
          width,
          "    answer: ".dim(),
          "            ".dim(),
          Style::default().fg(Color::Cyan),
        ));
      }
      if let Some(note) = note {
        let (label, continuation, style) = if question.options.is_some() {
          (
            "    note: ".dim(),
            "          ".dim(),
            Style::default().fg(Color::Cyan),
          )
        } else {
          (
            "    answer: ".dim(),
            "            ".dim(),
            Style::default().fg(Color::Cyan),
          )
        };
        lines.extend(wrap_with_prefix(&note, width, label, continuation, style));
      }
    }

    if self.interrupted && unanswered > 0 {
      let summary = format!("submitted with {unanswered} unanswered");
      lines.extend(wrap_with_prefix(
        &summary,
        width,
        "  ↳ ".dim().cyan(),
        "    ".dim(),
        Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
      ));
    }

    lines
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

fn wrap_with_prefix(
  text: &str,
  width: usize,
  initial_prefix: Span<'static>,
  subsequent_prefix: Span<'static>,
  style: Style,
) -> Vec<Line<'static>> {
  let prefix_width = UnicodeWidthStr::width(initial_prefix.content.as_ref())
    .max(UnicodeWidthStr::width(subsequent_prefix.content.as_ref()));
  let wrap_width = width.saturating_sub(prefix_width).max(1);
  let wrapped = textwrap::wrap(text, wrap_width);
  let lines = wrapped
    .into_iter()
    .map(|segment| Line::from(Span::styled(segment.to_string(), style)))
    .collect::<Vec<_>>();
  prefix_lines(lines, initial_prefix, subsequent_prefix)
}

fn split_request_user_input_answer(
  answer: &RequestUserInputAnswer,
) -> (Vec<String>, Option<String>) {
  let mut options = Vec::new();
  let mut note = None;
  for entry in &answer.answers {
    if let Some(note_text) = entry.strip_prefix("user_note: ") {
      note = Some(note_text.to_string());
    } else {
      options.push(entry.clone());
    }
  }
  (options, note)
}

fn take_prefix_by_width(text: &str, max_cols: usize) -> (String, &str, usize) {
  if max_cols == 0 || text.is_empty() {
    return (String::new(), text, 0);
  }

  let mut cols = 0usize;
  let mut end_idx = 0usize;
  for (i, ch) in text.char_indices() {
    let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
    if cols.saturating_add(ch_width) > max_cols {
      break;
    }
    cols += ch_width;
    end_idx = i + ch.len_utf8();
    if cols == max_cols {
      break;
    }
  }

  (text[..end_idx].to_string(), &text[end_idx..], cols)
}

impl HistoryCell for UserHistoryCell {
  fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
    let style = user_message_style();
    let element_style = style.fg(Color::Cyan);
    let use_explicit_box = style.bg.is_none();
    let wrap_width = if use_explicit_box {
      width.saturating_sub(4).max(1)
    } else {
      width.saturating_sub(3).max(1) // "› " prefix = 2 cols + 1 margin
    };

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

    if use_explicit_box {
      // Tradeoff: when terminal default colors are unavailable, bg-only styling
      // disappears in inline scrollback. We switch to an explicit bordered block
      // so user-authored messages still read as isolated "bubbles".
      let border_style = Style::default().dim();
      let horizontal = "─".repeat(width.saturating_sub(2) as usize);
      let mut content_lines: Vec<Line<'static>> = Vec::new();

      if let Some(imgs) = wrapped_remote_images {
        content_lines.extend(imgs);
        if wrapped_message.is_some() {
          content_lines.push(Line::from(""));
        }
      }

      if let Some(msg) = wrapped_message {
        content_lines.extend(msg);
      }

      let mut lines: Vec<Line<'static>> = vec![
        Line::from(""),
        Line::from(vec![
          Span::styled("╭".to_string(), border_style),
          Span::styled(horizontal.clone(), border_style),
          Span::styled("╮".to_string(), border_style),
        ]),
      ];
      lines.extend(prefix_lines(
        content_lines,
        Span::styled("│ ".to_string(), border_style),
        Span::styled("│ ".to_string(), border_style),
      ));
      lines.push(Line::from(vec![
        Span::styled("╰".to_string(), border_style),
        Span::styled(horizontal, border_style),
        Span::styled("╯".to_string(), border_style),
      ]));
      lines.push(Line::from(""));
      return lines;
    }

    let mut lines: Vec<Line<'static>> = vec![Line::from("").style(style)];

    if let Some(imgs) = wrapped_remote_images {
      lines.extend(prefix_lines(imgs, "  ".into(), "  ".into()));
      if wrapped_message.is_some() {
        lines.push(Line::from("").style(style));
      }
    }

    if let Some(msg) = wrapped_message {
      lines.extend(prefix_lines(msg, "› ".bold().dim(), "  ".into()));
    }

    lines.push(Line::from("").style(style));
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
  _is_first_session: bool,
) -> SessionInfoCell {
  // Tradeoff: we intentionally keep the session banner minimal in transcript
  // history. The richer welcome/help content still belongs to the empty-state
  // UI, but repeating it in committed history makes every fresh terminal launch
  // feel "dirty" compared with Claude Code.
  SessionInfoCell(CompositeHistoryCell::new(vec![Box::new(
    SessionHeaderHistoryCell::new(model, approval_policy, sandbox_mode, cwd),
  )]))
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
          "● ".dim().into()
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
        .initial_indent("● ".dim().into())
        .subsequent_indent("  ".into()),
    )
  }
}

#[derive(Debug)]
pub(crate) struct TurnCompleteHistoryCell {
  pub(crate) elapsed_seconds: Option<u64>,
  pub(crate) input_tokens: i64,
  pub(crate) output_tokens: i64,
}

impl HistoryCell for TurnCompleteHistoryCell {
  fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
    let mut label_parts = Vec::new();
    if let Some(elapsed_seconds) = self
      .elapsed_seconds
      .filter(|seconds| *seconds > 60)
      .map(fmt_elapsed_compact)
    {
      label_parts.push(format!("Worked for {elapsed_seconds}"));
    }
    if self.input_tokens > 0 || self.output_tokens > 0 {
      label_parts.push(format!(
        "{} in / {} out",
        self.input_tokens, self.output_tokens
      ));
    }

    if label_parts.is_empty() {
      return vec![Line::from("─".repeat(width as usize)).dim()];
    }

    let label = format!("─ {} ─", label_parts.join(" • "));
    let (label, _suffix, label_width) = take_prefix_by_width(&label, width as usize);
    vec![Line::from(vec![
      label.dim(),
      "─"
        .repeat((width as usize).saturating_sub(label_width))
        .dim(),
    ])]
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
  fn first_session_info_only_renders_header() {
    let cell = new_session_info(
      "openrouter/anthropic/claude-haiku-4.5".to_string(),
      "Ask".to_string(),
      "Permissive".to_string(),
      None,
      true,
    );

    let rendered = lines_to_string(&cell.display_lines(80));
    assert!(rendered.contains("┌─ cokra ─ openrouter/anthropic/claude-haiku-4.5"));
    assert!(!rendered.contains("Welcome to Cokra"));
    assert!(!rendered.contains("/help - show available commands"));
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

  #[test]
  fn request_user_input_result_cell_renders_answers_and_note() {
    let cell = RequestUserInputResultCell {
      questions: vec![
        RequestUserInputQuestion {
          id: "confirm".to_string(),
          header: "Confirm".to_string(),
          question: "Proceed with the plan?".to_string(),
          is_other: true,
          is_secret: false,
          options: Some(vec![cokra_protocol::RequestUserInputQuestionOption {
            label: "Yes".to_string(),
            description: "Continue.".to_string(),
          }]),
        },
        RequestUserInputQuestion {
          id: "secret".to_string(),
          header: "Secret".to_string(),
          question: "Enter token".to_string(),
          is_other: false,
          is_secret: true,
          options: None,
        },
      ],
      answers: HashMap::from([
        (
          "confirm".to_string(),
          RequestUserInputAnswer {
            answers: vec!["Yes".to_string(), "user_note: immediately".to_string()],
          },
        ),
        (
          "secret".to_string(),
          RequestUserInputAnswer {
            answers: vec!["super-secret".to_string()],
          },
        ),
      ]),
      interrupted: true,
    };

    let rendered = lines_to_string(&cell.display_lines(80));
    assert!(rendered.contains("Questions 2/2 answered"));
    assert!(rendered.contains("Proceed with the plan?"));
    assert!(rendered.contains("Yes"));
    assert!(rendered.contains("immediately"));
    assert!(rendered.contains("••••••"));
    assert!(rendered.contains("submitted early"));
  }

  #[test]
  fn turn_complete_history_cell_includes_elapsed_when_over_a_minute() {
    let cell = TurnCompleteHistoryCell {
      elapsed_seconds: Some(125),
      input_tokens: 321,
      output_tokens: 654,
    };

    let rendered = lines_to_string(&cell.display_lines(80));
    assert!(rendered.contains("Worked for 2m 05s"));
    assert!(rendered.contains("321 in / 654 out"));
  }

  #[test]
  fn turn_complete_history_cell_omits_short_elapsed_labels() {
    let cell = TurnCompleteHistoryCell {
      elapsed_seconds: Some(42),
      input_tokens: 1,
      output_tokens: 2,
    };

    let rendered = lines_to_string(&cell.display_lines(80));
    assert!(!rendered.contains("Worked for"));
    assert!(rendered.contains("1 in / 2 out"));
  }

  #[test]
  fn user_history_cell_adds_visual_spacing_block() {
    let cell = UserHistoryCell::from_text("hello world".to_string());
    let lines = cell.display_lines(80);
    let rendered = lines_to_string(&lines);

    assert!(lines.len() >= 3);
    assert!(
      lines
        .first()
        .is_some_and(|line| line.spans.iter().all(|span| span.content.is_empty()))
    );
    assert!(rendered.contains("hello world"));
    assert!(rendered.contains("╭") || rendered.contains("› hello world"));
    assert!(
      lines
        .last()
        .is_some_and(|line| line.spans.iter().all(|span| span.content.is_empty()))
    );
  }

  #[test]
  fn user_history_cell_falls_back_to_visible_box_when_no_terminal_bg() {
    let cell = UserHistoryCell::from_text("boxed inline message".to_string());
    let rendered = lines_to_string(&cell.display_lines(80));

    assert!(rendered.contains("╭"));
    assert!(rendered.contains("╰"));
    assert!(rendered.contains("boxed inline message"));
  }
}
