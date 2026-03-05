use std::time::Duration;
use std::time::Instant;

use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use super::model::CommandOutput;
use super::model::ExecCall;
use super::model::ExecCell;
use crate::history_cell::HistoryCell;
use crate::render::highlight::highlight_bash_to_lines;
use crate::render::line_utils::push_owned_lines;
use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_line;
use crate::wrapping::word_wrap_lines;

pub(crate) const TOOL_CALL_MAX_LINES: usize = 5;

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

  let visible = total.min(line_limit);
  for (idx, row) in rows.iter().take(visible).enumerate() {
    let prefix = if idx == 0 { prefix_head } else { prefix_next };
    out.push(Line::from(vec![
      Span::from(prefix).dim(),
      Span::from((*row).to_string()).dim(),
    ]));
  }

  let omitted = total.saturating_sub(visible);
  if omitted > 0 {
    out.push(Line::from(vec![
      Span::from("    ").dim(),
      Span::from(format!("... ({omitted} lines omitted)")).dim(),
    ]));
  }

  OutputLines {
    lines: out,
    omitted: (omitted > 0).then_some(omitted),
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
  fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    for (idx, call) in self.iter_calls().enumerate() {
      if idx > 0 {
        lines.push(Line::from(""));
      }

      let header_icon: Span<'static> = match (&call.output, call.duration) {
        (Some(output), Some(_)) => {
          if output.exit_code == 0 {
            "✓ ".green().bold()
          } else {
            "✗ ".red().bold()
          }
        }
        _ => {
          let mut s = crate::exec_cell::spinner(call.start_time, self.animations_enabled());
          s.content = format!("{} ", s.content).into();
          s
        }
      };
      let mut header = Line::from(vec![header_icon]);
      header.push_span(call.tool_name.clone().bold());
      // 1:1 codex: Do NOT display cwd in header - this was causing "unstable" display
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
        let cmd_line = Line::from(call.command.clone().dim());
        let wrapped_cmd = word_wrap_lines(
          &[cmd_line],
          RtOptions::new(width.max(1) as usize)
            .initial_indent("  › ".cyan().into())
            .subsequent_indent("    ".into()),
        );
        lines.extend(wrapped_cmd);
      }

      let rendered = output_lines(
        call.output.as_ref(),
        OutputLinesParams {
          line_limit: TOOL_CALL_MAX_LINES,
          only_err: false,
          include_angle_pipe: true,
          include_prefix: true,
        },
      );
      lines.extend(rendered.lines);

      match (&call.output, call.duration) {
        (Some(output), Some(duration)) => {
          let mut end = if output.exit_code == 0 {
            Line::from("  ✓".green().bold())
          } else {
            Line::from(vec![
              "  ✗".red().bold(),
              format!(" ({})", output.exit_code).into(),
            ])
          };
          end.push_span(format!(" • {}", format_duration_human(duration)).dim());
          lines.push(end);
        }
        _ => {
          lines.push(Line::from(vec![
            crate::exec_cell::spinner(call.start_time, self.animations_enabled()),
            " running".dim(),
          ]));
        }
      }
    }

    lines
  }

  fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
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
        let cmd_line = Line::from(call.command.clone().dim());
        let wrapped_cmd = word_wrap_lines(
          &[cmd_line],
          RtOptions::new(width.max(1) as usize)
            .initial_indent("› ".cyan().into())
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
      command: "read_file".to_string(),
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
      command: "list_dir".to_string(),
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
    let cell = ExecCell::new(completed_read_file_call(), false);
    let rendered = render_exec_cell(&cell, 60, 10);
    insta::assert_snapshot!(rendered);
  }

  #[test]
  fn snapshot_list_dir_tool_completed() {
    let cell = ExecCell::new(completed_list_dir_call(), false);
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
}
