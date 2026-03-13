use std::time::Duration;
use std::time::Instant;

use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use super::model::CommandOutput;
use super::model::ExecCall;
use super::model::ExecCell;
use crate::exec_cell::spinner;
use crate::history_cell::HistoryCell;
use crate::render::highlight::highlight_bash_to_lines;
use crate::render::line_utils::prefix_lines;
use crate::render::line_utils::push_owned_lines;
use crate::terminal_palette::light_blue;
use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_line;
use crate::wrapping::word_wrap_lines;

pub(crate) const TOOL_CALL_MAX_LINES: usize = 5;
const SHELL_TOOL_CALL_MAX_LINES: usize = 2;
const EXPLORING_SUMMARY_MAX_ITEMS: usize = 3;
const EXPLORING_LIVE_MAX_HEIGHT: u16 = 6;

#[derive(Debug, Clone, Copy)]
pub(crate) struct OutputLinesParams {
  pub(crate) line_limit: usize,
  pub(crate) only_err: bool,
  pub(crate) include_angle_pipe: bool,
  pub(crate) include_prefix: bool,
}

#[derive(Clone)]
pub(crate) struct OutputLines {
  pub(crate) lines: Vec<Line<'static>>,
  pub(crate) omitted: Option<usize>,
}

pub(crate) fn new_active_exec_command(
  command_id: String,
  tool_name: String,
  command: String,
  cwd: std::path::PathBuf,
  animations_enabled: bool,
) -> ExecCell {
  ExecCell::new(
    ExecCall {
      command_id,
      tool_name,
      command,
      cwd,
      output: None,
      start_time: Some(Instant::now()),
      duration: None,
    },
    animations_enabled,
  )
}

pub(crate) fn output_lines(
  output: Option<&CommandOutput>,
  params: OutputLinesParams,
) -> OutputLines {
  let OutputLinesParams {
    line_limit,
    only_err,
    include_angle_pipe,
    include_prefix,
  } = params;

  let Some(output) = output else {
    return OutputLines {
      lines: Vec::new(),
      omitted: None,
    };
  };

  if only_err && output.exit_code == 0 {
    return OutputLines {
      lines: Vec::new(),
      omitted: None,
    };
  }

  let rows: Vec<&str> = output.output.lines().collect();
  let total = rows.len();
  let mut out = Vec::new();
  let prefix_head = if include_prefix && include_angle_pipe {
    "  └ "
  } else if include_prefix {
    "    "
  } else {
    ""
  };
  let prefix_next = if include_prefix { "    " } else { "" };

  let head_end = total.min(line_limit);
  for (idx, row) in rows.iter().take(head_end).enumerate() {
    let prefix = if idx == 0 { prefix_head } else { prefix_next };
    out.push(Line::from(vec![
      Span::from(prefix).dim(),
      Span::from((*row).to_string()).dim(),
    ]));
  }

  let show_ellipsis = total > 2 * line_limit;
  let omitted = if show_ellipsis {
    total.saturating_sub(2 * line_limit)
  } else {
    0
  };
  if show_ellipsis {
    out.push(Line::from(vec![
      Span::from(prefix_next).dim(),
      Span::from(format!("… +{omitted} lines")).dim(),
    ]));
  }

  let tail_start = if show_ellipsis {
    total.saturating_sub(line_limit)
  } else {
    head_end
  };
  for row in rows.iter().skip(tail_start) {
    out.push(Line::from(vec![
      Span::from(prefix_next).dim(),
      Span::from((*row).to_string()).dim(),
    ]));
  }

  OutputLines {
    lines: out,
    omitted: show_ellipsis.then_some(omitted),
  }
}

fn format_duration_human(duration: Duration) -> String {
  let secs = duration.as_secs();
  if secs < 60 {
    return format!("{secs}s");
  }
  let mins = secs / 60;
  let rem = secs % 60;
  if mins < 60 {
    return format!("{mins}m {rem:02}s");
  }
  let hours = mins / 60;
  let rem_mins = mins % 60;
  format!("{hours}h {rem_mins:02}m {rem:02}s")
}

impl HistoryCell for ExecCell {
  fn is_stream_continuation(&self) -> bool {
    self.is_continuation
  }

  fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
    if self.is_exploring_cell() {
      return self.exploring_display_lines(width);
    }

    let mut lines = Vec::new();

    for (idx, call) in self.iter_calls().enumerate() {
      if idx > 0 {
        lines.push(Line::from(""));
      }

      // 1:1 codex: Show spinner during execution, ✓/✗ after completion
      // Also add "Running" text during execution
      let (header_icon, status_text, duration_ms): (Span<'static>, &str, Option<u128>) =
        match (&call.output, call.duration) {
          (Some(output), Some(dur)) => {
            let ms = dur.as_millis();
            if output.exit_code == 0 {
              ("✓ ".green().bold(), "", Some(ms))
            } else {
              ("✗ ".red().bold(), "", Some(ms))
            }
          }
          _ => {
            let mut s = crate::exec_cell::spinner(call.start_time, self.animations_enabled());
            s.content = format!("{} ", s.content).into();
            (s, "Running", None)
          }
        };
      let mut header = Line::from(vec![header_icon]);
      header.push_span(call.tool_name.clone().bold());
      if !status_text.is_empty() {
        header.push_span(format!(" {status_text}").dim());
      }
      if let Some(ms) = duration_ms {
        header.push_span(format!(" ({ms}ms)").dim());
      }
      lines.push(header);

      // Shell tool: render command with "$ " prefix and bash highlighting.
      // Non-shell tools: render the command/args with "› " prefix, no bash highlighting.
      if call.tool_name == "shell" {
        let highlighted = highlight_bash_to_lines(&call.command);
        let wrapped_cmd = word_wrap_lines(
          &highlighted,
          RtOptions::new(width.max(1) as usize)
            .initial_indent("  $ ".magenta().into())
            .subsequent_indent("    ".into()),
        );
        lines.extend(wrapped_cmd);
      } else {
        let cmd_line = Line::from(call.command.clone());
        let wrapped_cmd = word_wrap_lines(
          &[cmd_line],
          RtOptions::new(width.max(1) as usize)
            .initial_indent("  › ".into())
            .subsequent_indent("    ".into()),
        );
        lines.extend(wrapped_cmd);
      }

      let line_limit = if call.tool_name == "shell" {
        SHELL_TOOL_CALL_MAX_LINES
      } else {
        TOOL_CALL_MAX_LINES
      };
      let rendered = output_lines(
        call.output.as_ref(),
        OutputLinesParams {
          line_limit,
          only_err: false,
          include_angle_pipe: true,
          include_prefix: true,
        },
      );
      lines.extend(rendered.lines);

      // 1:1 codex: Don't show trailing "✓ • Xs" - redundant and not in codex
    }

    lines
  }

  fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
    if self.is_exploring_cell() {
      return self.exploring_display_lines(width);
    }

    let mut lines = Vec::new();
    for (idx, call) in self.iter_calls().enumerate() {
      if idx > 0 {
        lines.push(Line::from(""));
      }

      if call.tool_name == "shell" {
        let highlighted = highlight_bash_to_lines(&call.command);
        let wrapped_cmd = word_wrap_lines(
          &highlighted,
          RtOptions::new(width.max(1) as usize)
            .initial_indent("$ ".magenta().into())
            .subsequent_indent("  ".into()),
        );
        lines.extend(wrapped_cmd);
      } else {
        let cmd_line = Line::from(call.command.clone());
        let wrapped_cmd = word_wrap_lines(
          &[cmd_line],
          RtOptions::new(width.max(1) as usize)
            .initial_indent("› ".into())
            .subsequent_indent("  ".into()),
        );
        lines.extend(wrapped_cmd);
      }

      if let Some(output) = call.output.as_ref() {
        for raw in output.output.lines() {
          let line = Line::from(raw.to_string());
          let wrapped = word_wrap_line(&line, RtOptions::new(width.max(1) as usize));
          push_owned_lines(&wrapped, &mut lines);
        }
      }
    }
    lines
  }
}

impl ExecCell {
  fn exploring_summary_item_blocks(&self, width: u16) -> Vec<Vec<Line<'static>>> {
    let mut summary_items = Vec::new();
    let mut idx = 0usize;
    while idx < self.calls.len() {
      let call = &self.calls[idx];
      if call.tool_name == "read_file" {
        let mut names = vec![call.command.clone()];
        idx += 1;
        while idx < self.calls.len() && self.calls[idx].tool_name == "read_file" {
          let next_name = self.calls[idx].command.clone();
          if !names.iter().any(|name| name == &next_name) {
            names.push(next_name);
          }
          idx += 1;
        }
        summary_items.push(wrap_exploring_line(width, "Read", names.join(", ")));
        continue;
      }

      let title = match call.tool_name.as_str() {
        "list_dir" => "List",
        "grep_files" | "search_tool" | "code_search" => "Search",
        "glob" => "Glob",
        "read_many_files" => "Read",
        _ => "Run",
      };
      summary_items.push(wrap_exploring_line(width, title, call.command.clone()));
      idx += 1;
    }
    summary_items
  }

  fn exploring_summary_lines(&self, width: u16, max_lines: Option<usize>) -> Vec<Line<'static>> {
    let all_items = self.exploring_summary_item_blocks(width);
    let total = all_items.len();
    let omitted_by_default = total.saturating_sub(EXPLORING_SUMMARY_MAX_ITEMS);
    let visible_items = all_items
      .into_iter()
      .skip(omitted_by_default)
      .collect::<Vec<_>>();

    let kept_items = if let Some(limit) = max_lines {
      if limit == 0 {
        Vec::new()
      } else {
        let mut available_for_items = limit;
        let mut kept_rev = Vec::new();
        loop {
          kept_rev.clear();
          let mut remaining = available_for_items;
          for block in visible_items.iter().rev() {
            if remaining == 0 {
              break;
            }
            if block.len() <= remaining {
              kept_rev.push(block.clone());
              remaining -= block.len();
            } else if kept_rev.is_empty() {
              kept_rev.push(block.iter().take(remaining).cloned().collect());
              remaining = 0;
            } else {
              break;
            }
          }

          let kept_len = kept_rev.len();
          let omitted = total.saturating_sub(kept_len);
          if omitted == 0 || available_for_items == 0 || limit == 1 {
            break kept_rev.into_iter().rev().collect::<Vec<_>>();
          }

          let reserved_for_omitted = limit.saturating_sub(1);
          if available_for_items == reserved_for_omitted {
            break kept_rev.into_iter().rev().collect::<Vec<_>>();
          }
          available_for_items = reserved_for_omitted;
        }
      }
    } else {
      visible_items
    };

    let omitted = total.saturating_sub(kept_items.len());
    let mut summary_lines = Vec::new();
    if omitted > 0 {
      summary_lines.push(Line::from(Span::from(format!("… +{omitted} more")).dim()));
    }
    summary_lines.extend(kept_items.into_iter().flatten());
    prefix_lines(summary_lines, "  └ ".dim(), "    ".into())
  }

  pub(crate) fn live_transcript_lines(&self, width: u16, max_height: u16) -> Vec<Line<'static>> {
    if !self.is_exploring_cell() {
      return self.transcript_lines(width);
    }

    let mut lines = Vec::new();
    let is_live = self.is_active();
    lines.push(Line::from(vec![
      if is_live {
        spinner(self.exploring_since, self.animations_enabled())
      } else {
        "●".dim()
      },
      " ".into(),
      if is_live {
        Span::from("Exploring").style(Style::new().fg(light_blue()).add_modifier(Modifier::BOLD))
      } else {
        Span::from("Explored").style(Style::new().fg(light_blue()).add_modifier(Modifier::BOLD))
      },
    ]));

    let remaining_lines = max_height.saturating_sub(1) as usize;
    lines.extend(self.exploring_summary_lines(width, Some(remaining_lines)));
    lines
  }

  pub(crate) fn live_desired_height(&self, width: u16) -> u16 {
    self
      .live_transcript_lines(width, EXPLORING_LIVE_MAX_HEIGHT)
      .len()
      .try_into()
      .unwrap_or(EXPLORING_LIVE_MAX_HEIGHT)
  }

  fn exploring_display_lines(&self, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    // Explore groups should only show a spinner while at least one call in the
    // group is still active. Once the last call completes, the cell remains
    // grouped but immediately flips to "Explored" until a later explore call
    // reactivates the same group.
    let is_live = self.is_active();
    lines.push(Line::from(vec![
      if is_live {
        spinner(self.exploring_since, self.animations_enabled())
      } else {
        "●".dim()
      },
      " ".into(),
      if is_live {
        Span::from("Exploring").style(Style::new().fg(light_blue()).add_modifier(Modifier::BOLD))
      } else {
        Span::from("Explored").style(Style::new().fg(light_blue()).add_modifier(Modifier::BOLD))
      },
    ]));

    lines.extend(self.exploring_summary_lines(width, None));
    lines
  }
}

fn wrap_exploring_line(width: u16, title: &str, content: String) -> Vec<Line<'static>> {
  let width = width.max(1) as usize;
  // "  └ " / "    " prefix added by prefix_lines is 4 chars wide.
  const OUTER_PREFIX_WIDTH: usize = 4;
  // Continuation indent aligns text after "<Title> " on the first line.
  // The outer prefix_lines subsequent span ("    ") already provides 4 chars,
  // so the inner continuation only needs (title.len() + 1) extra spaces.
  let continuation_extra = " ".repeat(title.len() + 1);
  let first_line_prefix_width = OUTER_PREFIX_WIDTH + title.len() + 1;
  let available_width = width.saturating_sub(first_line_prefix_width).max(1);
  let wrapped = textwrap::wrap(&content, available_width);

  wrapped
    .into_iter()
    .enumerate()
    .map(|(idx, line)| {
      let prefix = if idx == 0 {
        Line::from(vec![
          Span::from(title.to_string()).style(light_blue()),
          " ".into(),
        ])
      } else {
        continuation_extra.clone().into()
      };
      let mut out = prefix;
      out.push_span(line.into_owned());
      out
    })
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;
  use ratatui::Terminal;
  use ratatui::backend::TestBackend;
  use ratatui::widgets::Paragraph;
  use ratatui::widgets::Widget;
  use std::path::PathBuf;
  use std::time::Duration;

  /// Helper: render an ExecCell into a test terminal and return the backend string.
  fn render_exec_cell(cell: &ExecCell, width: u16, height: u16) -> String {
    let lines = cell.display_lines(width);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
      .draw(|f| Paragraph::new(lines).render(f.area(), f.buffer_mut()))
      .expect("draw");
    format!("{}", terminal.backend())
  }

  fn completed_shell_call() -> ExecCall {
    ExecCall {
      command_id: "call-1".to_string(),
      tool_name: "shell".to_string(),
      command: "ls -la /tmp".to_string(),
      cwd: PathBuf::from("/home/user/project"),
      output: Some(CommandOutput {
        exit_code: 0,
        output: "total 8\ndrwxrwxrwt 2 root root 40 Jan  1 00:00 .\n".to_string(),
      }),
      start_time: None,
      duration: Some(Duration::from_millis(42)),
    }
  }

  fn completed_read_file_call() -> ExecCall {
    ExecCall {
      command_id: "call-2".to_string(),
      tool_name: "read_file".to_string(),
      command: "main.rs".to_string(),
      cwd: PathBuf::from("/home/user/project"),
      output: Some(CommandOutput {
        exit_code: 0,
        output: "1: fn main() {\n2:   println!(\"hello\");\n3: }\n".to_string(),
      }),
      start_time: None,
      duration: Some(Duration::from_millis(5)),
    }
  }

  fn completed_list_dir_call() -> ExecCall {
    ExecCall {
      command_id: "call-3".to_string(),
      tool_name: "list_dir".to_string(),
      command: "src".to_string(),
      cwd: PathBuf::from("/home/user/project"),
      output: Some(CommandOutput {
        exit_code: 0,
        output: "src/\nCargo.toml\nREADME.md\n".to_string(),
      }),
      start_time: None,
      duration: Some(Duration::from_millis(3)),
    }
  }

  #[test]
  fn snapshot_shell_tool_completed() {
    let cell = ExecCell::new(completed_shell_call(), false);
    let rendered = render_exec_cell(&cell, 60, 10);
    insta::assert_snapshot!(rendered);
  }

  #[test]
  fn snapshot_read_file_tool_completed() {
    // Use scrollback_snapshot() to render the post-flush "Explored" state
    // (exploring_since = None). Live cells only show "Exploring" while active.
    let cell = ExecCell::new(completed_read_file_call(), false).scrollback_snapshot();
    let rendered = render_exec_cell(&cell, 60, 10);
    insta::assert_snapshot!(rendered);
  }

  #[test]
  fn snapshot_list_dir_tool_completed() {
    // Same: test the scrollback "Explored" rendering.
    let cell = ExecCell::new(completed_list_dir_call(), false).scrollback_snapshot();
    let rendered = render_exec_cell(&cell, 60, 10);
    insta::assert_snapshot!(rendered);
  }

  #[test]
  fn snapshot_mixed_tools_in_one_cell() {
    let mut cell = ExecCell::new(completed_shell_call(), false);
    cell.push_call(completed_read_file_call());
    cell.push_call(completed_list_dir_call());
    let rendered = render_exec_cell(&cell, 60, 25);
    insta::assert_snapshot!(rendered);
  }

  #[test]
  fn snapshot_failed_shell_tool() {
    let call = ExecCall {
      command_id: "call-err".to_string(),
      tool_name: "shell".to_string(),
      command: "cat /nonexistent".to_string(),
      cwd: PathBuf::from("/home/user"),
      output: Some(CommandOutput {
        exit_code: 1,
        output: "cat: /nonexistent: No such file or directory\n".to_string(),
      }),
      start_time: None,
      duration: Some(Duration::from_millis(10)),
    };
    let cell = ExecCell::new(call, false);
    let rendered = render_exec_cell(&cell, 60, 8);
    insta::assert_snapshot!(rendered);
  }

  #[test]
  fn shell_output_uses_compact_preview_limit() {
    let output = (1..=10)
      .map(|n| format!("line {n}"))
      .collect::<Vec<_>>()
      .join("\n");
    let call = ExecCall {
      command_id: "call-many".to_string(),
      tool_name: "shell".to_string(),
      command: "printf 'many lines'".to_string(),
      cwd: PathBuf::from("/home/user"),
      output: Some(CommandOutput {
        exit_code: 0,
        output,
      }),
      start_time: None,
      duration: Some(Duration::from_millis(10)),
    };

    let cell = ExecCell::new(call, false);
    let lines = cell.display_lines(80);
    let rendered = lines.iter().map(Line::to_string).collect::<Vec<_>>();

    assert!(rendered.iter().any(|line| line.contains("line 1")));
    assert!(rendered.iter().any(|line| line.contains("line 2")));
    assert!(rendered.iter().any(|line| line.contains("… +6 lines")));
    assert!(rendered.iter().any(|line| line.contains("line 9")));
    assert!(rendered.iter().any(|line| line.contains("line 10")));
    assert!(
      !rendered.iter().any(|line| line.contains("line 3")),
      "shell preview should not keep more than two head lines"
    );
    assert!(
      !rendered.iter().any(|line| line.contains("line 8")),
      "shell preview should not keep more than two tail lines"
    );
  }

  #[test]
  fn exploring_summary_caps_visible_items() {
    let mut cell = ExecCell::new(
      ExecCall {
        command_id: "call-1".to_string(),
        tool_name: "code_search".to_string(),
        command: "agentteams".to_string(),
        cwd: PathBuf::from("/home/user/project"),
        output: Some(CommandOutput {
          exit_code: 0,
          output: "{}".to_string(),
        }),
        start_time: None,
        duration: Some(Duration::from_millis(1)),
      },
      false,
    );
    cell.push_call(ExecCall {
      command_id: "call-2".to_string(),
      tool_name: "code_search".to_string(),
      command: "spawn_agent".to_string(),
      cwd: PathBuf::from("/home/user/project"),
      output: Some(CommandOutput {
        exit_code: 0,
        output: "{}".to_string(),
      }),
      start_time: None,
      duration: Some(Duration::from_millis(1)),
    });
    cell.push_call(ExecCall {
      command_id: "call-3".to_string(),
      tool_name: "code_search".to_string(),
      command: "team_status".to_string(),
      cwd: PathBuf::from("/home/user/project"),
      output: Some(CommandOutput {
        exit_code: 0,
        output: "{}".to_string(),
      }),
      start_time: None,
      duration: Some(Duration::from_millis(1)),
    });
    cell.push_call(ExecCall {
      command_id: "call-4".to_string(),
      tool_name: "code_search".to_string(),
      command: "cleanup_team".to_string(),
      cwd: PathBuf::from("/home/user/project"),
      output: Some(CommandOutput {
        exit_code: 0,
        output: "{}".to_string(),
      }),
      start_time: None,
      duration: Some(Duration::from_millis(1)),
    });

    let rendered = cell
      .display_lines(80)
      .iter()
      .map(Line::to_string)
      .collect::<Vec<_>>();

    assert!(
      rendered
        .iter()
        .any(|line| line.contains("Search spawn_agent"))
    );
    assert!(
      rendered
        .iter()
        .any(|line| line.contains("Search team_status"))
    );
    assert!(
      rendered
        .iter()
        .any(|line| line.contains("Search cleanup_team"))
    );
    assert!(rendered.iter().any(|line| line.contains("… +1 more")));
    assert!(
      !rendered.iter().any(|line| line.contains("agentteams")),
      "exploring summary should hide earliest items, showing only the latest 3"
    );
  }
}
