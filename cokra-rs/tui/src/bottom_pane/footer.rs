use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FooterMode {
  Default,
  TaskRunning,
  Plan,
  QuitShortcutReminder,
  UserShell,
}

#[derive(Clone, Debug)]
pub(crate) struct FooterProps {
  pub(crate) mode: FooterMode,
  pub(crate) esc_backtrack_hint: bool,
  pub(crate) use_shift_enter_hint: bool,
  pub(crate) is_task_running: bool,
  pub(crate) collaboration_modes_enabled: bool,
  pub(crate) context_window_percent: Option<i64>,
  pub(crate) context_window_used_tokens: Option<i64>,
  pub(crate) status_line_value: Option<Line<'static>>,
  pub(crate) status_line_enabled: bool,
}

impl Default for FooterProps {
  fn default() -> Self {
    Self {
      mode: FooterMode::Default,
      esc_backtrack_hint: false,
      use_shift_enter_hint: true,
      is_task_running: false,
      collaboration_modes_enabled: false,
      context_window_percent: None,
      context_window_used_tokens: None,
      status_line_value: None,
      status_line_enabled: false,
    }
  }
}

pub(crate) fn render_footer_from_props(props: &FooterProps, area: Rect, buf: &mut Buffer) {
  if area.height == 0 {
    return;
  }

  let mode_label = match props.mode {
    FooterMode::Default => "Enter submit  |  Shift+Enter newline  |  Esc quit",
    FooterMode::TaskRunning => "Task running  |  Enter queue  |  Ctrl+C interrupt",
    FooterMode::Plan => "Plan mode",
    FooterMode::QuitShortcutReminder => "Press Esc again to quit",
    FooterMode::UserShell => "User shell mode",
  };

  let mut line = Line::from(mode_label.dim());
  if let Some(percent) = props.context_window_percent {
    line.push_span("  |  ".dim());
    line.push_span(format!("ctx {percent}%").dim());
  }

  Paragraph::new(line).render(area, buf);
}
