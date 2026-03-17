// 1:1 codex: Bordered status card for /status slash command.

use std::path::PathBuf;

use ratatui::prelude::*;
use ratatui::style::Stylize;

use crate::history_cell::CompositeHistoryCell;
use crate::history_cell::HistoryCell;
use crate::history_cell::PlainHistoryCell;
use crate::history_cell::with_border_with_inner_width;

use super::format::FieldFormatter;
use super::format::line_display_width;
use super::format::truncate_line_to_width;

/// Data bag passed from App to build the status card.
#[derive(Debug, Clone)]
pub(crate) struct StatusCardData {
  pub(crate) model_name: String,
  pub(crate) directory: PathBuf,
  pub(crate) session_id: String,
  pub(crate) task_running: bool,
  pub(crate) input_tokens: i64,
  pub(crate) output_tokens: i64,
  pub(crate) total_tokens: i64,
  pub(crate) collaboration_mode: Option<String>,
  pub(crate) agents_count: Option<usize>,
}

pub(crate) fn new_status_output(data: StatusCardData) -> CompositeHistoryCell {
  let command = PlainHistoryCell::new(vec!["/status".magenta().into()]);
  let card = StatusHistoryCell::new(data);
  CompositeHistoryCell::new(vec![Box::new(command), Box::new(card)])
}

#[derive(Debug)]
struct StatusHistoryCell {
  data: StatusCardData,
}

impl StatusHistoryCell {
  fn new(data: StatusCardData) -> Self {
    Self { data }
  }
}

fn format_tokens_compact(tokens: i64) -> String {
  if tokens.abs() >= 1_000_000 {
    format!("{:.1}M", tokens as f64 / 1_000_000.0)
  } else if tokens.abs() >= 1_000 {
    format!("{:.1}K", tokens as f64 / 1_000.0)
  } else {
    tokens.to_string()
  }
}

fn format_directory_display(path: &PathBuf) -> String {
  // Try to show ~ for home directory
  if let Some(home) = dirs::home_dir() {
    if let Ok(stripped) = path.strip_prefix(&home) {
      return format!("~/{}", stripped.display());
    }
  }
  path.display().to_string()
}

impl HistoryCell for StatusHistoryCell {
  fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
    let d = &self.data;

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Header
    lines.push(Line::from(vec![
      Span::from(format!("{}>_ ", FieldFormatter::INDENT)).dim(),
      Span::from("Cokra AI").bold(),
      Span::from(" (v0.1.0)").dim(),
    ]));
    lines.push(Line::from(Vec::<Span<'static>>::new()));

    let available_inner_width = usize::from(width.saturating_sub(4));
    if available_inner_width == 0 {
      return Vec::new();
    }

    // Build labels
    let mut labels: Vec<&str> = vec!["Model", "Directory", "Session"];
    if d.collaboration_mode.is_some() {
      labels.push("Collaboration mode");
    }
    if d.agents_count.is_some() {
      labels.push("Agents");
    }
    labels.push("Task running");
    labels.push("Token usage");

    let formatter = FieldFormatter::from_labels(labels.iter().copied());

    // Fields
    lines.push(formatter.line("Model", vec![Span::from(d.model_name.clone())]));
    lines.push(formatter.line(
      "Directory",
      vec![Span::from(format_directory_display(&d.directory))],
    ));
    lines.push(formatter.line(
      "Session",
      vec![Span::from(d.session_id.clone()).dim()],
    ));

    if let Some(collab) = d.collaboration_mode.as_ref() {
      lines.push(formatter.line(
        "Collaboration mode",
        vec![Span::from(collab.clone())],
      ));
    }

    if let Some(count) = d.agents_count {
      lines.push(formatter.line(
        "Agents",
        vec![Span::from(format!("{count} active"))],
      ));
    }

    lines.push(formatter.line(
      "Task running",
      if d.task_running {
        vec![Span::from("yes").green()]
      } else {
        vec![Span::from("no").dim()]
      },
    ));

    // Token usage
    lines.push(Line::from(Vec::<Span<'static>>::new()));
    let total_fmt = format_tokens_compact(d.total_tokens);
    let input_fmt = format_tokens_compact(d.input_tokens);
    let output_fmt = format_tokens_compact(d.output_tokens);
    lines.push(formatter.line(
      "Token usage",
      vec![
        Span::from(format!("{total_fmt} total")),
        Span::from("  (").dim(),
        Span::from(format!("{input_fmt} input")).dim(),
        Span::from(" + ").dim(),
        Span::from(format!("{output_fmt} output")).dim(),
        Span::from(")").dim(),
      ],
    ));

    // Truncate and border
    let content_width = lines.iter().map(line_display_width).max().unwrap_or(0);
    let inner_width = content_width.min(available_inner_width);
    let truncated_lines: Vec<Line<'static>> = lines
      .into_iter()
      .map(|line| truncate_line_to_width(line, inner_width))
      .collect();

    with_border_with_inner_width(truncated_lines, inner_width)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn sample_data() -> StatusCardData {
    StatusCardData {
      model_name: "gpt-4.1".to_string(),
      directory: PathBuf::from("/home/user/project"),
      session_id: "019cfa84-716c-73c3".to_string(),
      task_running: false,
      input_tokens: 299_000,
      output_tokens: 17_400,
      total_tokens: 317_000,
      collaboration_mode: None,
      agents_count: None,
    }
  }

  #[test]
  fn status_card_has_border() {
    let data = sample_data();
    let cell = StatusHistoryCell::new(data);
    let lines = cell.display_lines(80);
    let rendered: Vec<String> = lines.iter().map(|l| l.to_string()).collect();

    assert!(
      rendered.first().is_some_and(|l| l.contains('╭')),
      "first line should have top border: {rendered:?}"
    );
    assert!(
      rendered.last().is_some_and(|l| l.contains('╰')),
      "last line should have bottom border: {rendered:?}"
    );
  }

  #[test]
  fn status_card_contains_model() {
    let data = sample_data();
    let cell = StatusHistoryCell::new(data);
    let lines = cell.display_lines(80);
    let rendered: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    assert!(
      rendered.iter().any(|l| l.contains("gpt-4.1")),
      "should contain model name: {rendered:?}"
    );
  }

  #[test]
  fn status_card_contains_tokens() {
    let data = sample_data();
    let cell = StatusHistoryCell::new(data);
    let lines = cell.display_lines(80);
    let rendered: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    assert!(
      rendered.iter().any(|l| l.contains("317.0K total")),
      "should contain formatted token total: {rendered:?}"
    );
  }

  #[test]
  fn status_card_includes_collab_mode_when_present() {
    let mut data = sample_data();
    data.collaboration_mode = Some("Agent Teams".to_string());
    data.agents_count = Some(3);
    let cell = StatusHistoryCell::new(data);
    let lines = cell.display_lines(80);
    let rendered: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    assert!(
      rendered.iter().any(|l| l.contains("Agent Teams")),
      "should contain collab mode: {rendered:?}"
    );
    assert!(
      rendered.iter().any(|l| l.contains("3 active")),
      "should contain agents count: {rendered:?}"
    );
  }

  #[test]
  fn format_tokens_compact_covers_ranges() {
    assert_eq!(format_tokens_compact(500), "500");
    assert_eq!(format_tokens_compact(1_500), "1.5K");
    assert_eq!(format_tokens_compact(1_500_000), "1.5M");
  }

  #[test]
  fn composite_status_output_has_command_and_card() {
    let data = sample_data();
    let composite = new_status_output(data);
    let lines = composite.display_lines(80);
    let rendered: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    // Command line
    assert!(
      rendered.iter().any(|l| l.contains("/status")),
      "should contain /status command: {rendered:?}"
    );
    // Border
    assert!(
      rendered.iter().any(|l| l.contains('╭')),
      "should contain bordered card: {rendered:?}"
    );
  }
}
