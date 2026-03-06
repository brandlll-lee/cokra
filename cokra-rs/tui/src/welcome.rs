//! Welcome screen with cokra logo - 1:1 port from codex onboarding welcome.
//!
//! This module displays the cokra ASCII logo on startup.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

use crate::history_cell::HistoryCell;
use crate::history_cell::PlainHistoryCell;

/// The cokra ASCII logo (纯白色极简风格)
/// 必须精确匹配原始logo，1:1复刻
const COKRA_LOGO: &[&str] = &[
  "░█▀▀░█▀█░█░█░█▀▄░█▀█",
  "░█░░░█░█░█▀▄░█▀▄░█▀█",
  "░▀▀▀░▀▀▀░▀░▀░▀░▀░▀░▀",
];

/// Welcome widget that displays the cokra logo and welcome message.
/// 1:1复刻codex的左对齐布局方式
pub(crate) struct WelcomeWidget {
  /// 是否显示"Press Enter to continue..."提示
  show_hint: bool,
}

impl WelcomeWidget {
  /// Create a new welcome widget with hint.
  pub(crate) fn new() -> Self {
    Self { show_hint: true }
  }

  /// Create a welcome widget without the hint (for permanent display).
  pub(crate) fn without_hint() -> Self {
    Self { show_hint: false }
  }

  /// Generate the welcome lines (used by both widget and history cell)
  fn generate_lines(show_hint: bool) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();

    // 添加空行作为顶部间距
    lines.push("".into());

    // 渲染logo - 左对齐，纯白色
    for line in COKRA_LOGO {
      lines.push(Line::from(vec![
        "  ".into(),         // 两个空格的左缩进（与codex一致）
        line.white().bold(), // 纯白色，加粗
      ]));
    }

    // 添加空行分隔
    lines.push("".into());

    // 欢迎文本 - 左对齐（1:1复刻codex的布局）
    lines.push(Line::from(vec![
      "  ".into(),                              // 两个空格的左缩进
      "Welcome to ".into(),                     // 普通文本
      "Cokra".bold(),                           // 加粗的Cokra
      ", AI Agent Team CLI Environment".into(), // 副标题
    ]));

    // 添加空行
    lines.push("".into());

    // 底部提示 - 左对齐（仅在初始显示时出现）
    if show_hint {
      lines.push(Line::from(vec![
        "  ".into(),                        // 两个空格的左缩进
        "Press Enter to continue...".dim(), // 暗色提示
      ]));
    }

    lines
  }

  /// Create a welcome header cell for permanent display in history
  pub(crate) fn into_history_cell() -> Box<dyn HistoryCell> {
    let mut lines = Self::generate_lines(false); // 不显示hint
    while lines
      .last()
      .is_some_and(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
    {
      lines.pop();
    }
    Box::new(PlainHistoryCell::new(lines))
  }
}

impl Widget for WelcomeWidget {
  fn render(self, area: Rect, buf: &mut Buffer) {
    // 清除区域（与codex一致）
    Clear.render(area, buf);

    // 生成lines并渲染
    let lines = Self::generate_lines(self.show_hint);

    // 使用Paragraph渲染，自动左对齐（与codex完全一致）
    Paragraph::new(lines)
      .wrap(Wrap { trim: false })
      .render(area, buf);
  }
}
