//! Welcome screen with cokra logo.
//!
//! Cokra preserves all inline scrollback history, including startup onboarding.
//! We therefore commit the welcome banner as the first transcript history cell
//! so it remains visible after later turns and session events.

use cokra_config::Config;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
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

#[derive(Debug, Clone)]
pub(crate) struct WelcomeContext {
  pub(crate) model_id: String,
  pub(crate) sandbox_mode: String,
  pub(crate) approval_mode: String,
}

impl WelcomeContext {
  pub(crate) fn from_config(cfg: &Config) -> Self {
    Self {
      model_id: format_model_id(&cfg.models.provider, &cfg.models.model),
      sandbox_mode: format!("{:?}", cfg.sandbox.mode),
      approval_mode: format!("{:?}", cfg.approval.policy),
    }
  }
}

fn format_model_id(provider: &str, model: &str) -> String {
  let provider = provider.trim();
  let model = model.trim();
  if provider.is_empty() {
    return model.to_string();
  }
  if model.is_empty() {
    return provider.to_string();
  }
  let prefix = format!("{provider}/");
  if model.starts_with(&prefix) {
    return model.to_string();
  }
  format!("{provider}/{model}")
}

/// Welcome widget that displays the cokra logo and welcome message.
/// 1:1复刻codex的左对齐布局方式
pub(crate) struct WelcomeWidget {
  /// 是否显示"Press Enter to continue..."提示
  show_hint: bool,
  ctx: WelcomeContext,
}

impl WelcomeWidget {
  /// Create a new welcome widget with hint.
  pub(crate) fn new(ctx: WelcomeContext) -> Self {
    Self {
      show_hint: true,
      ctx,
    }
  }

  /// Create a welcome widget without the hint (for permanent display).
  pub(crate) fn without_hint(ctx: WelcomeContext) -> Self {
    Self {
      show_hint: false,
      ctx,
    }
  }

  /// Generate the welcome lines (used by both widget and history cell)
  fn generate_lines(ctx: &WelcomeContext, show_hint: bool) -> Vec<Line<'static>> {
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
      "  ".into(),
      Span::raw("Welcome to "),
      Span::styled("Cokra", ratatui::style::Style::new().bold()),
      Span::raw(", AI Agent Team CLI Environment"),
    ]));

    // 添加空行
    lines.push("".into());

    // Status block (startup header)
    lines.push(Line::from(vec![
      "  ".into(),
      Span::raw("┌─ cokra ─ "),
      Span::styled(ctx.model_id.clone(), ratatui::style::Style::new().bold()),
    ]));
    lines.push(Line::from(vec![
      "  ".into(),
      Span::raw("│  sandbox: "),
      Span::styled(
        ctx.sandbox_mode.clone(),
        ratatui::style::Style::new().bold(),
      ),
      Span::raw(" │ approval: "),
      Span::styled(
        ctx.approval_mode.clone(),
        ratatui::style::Style::new().bold(),
      ),
    ]));
    lines.push(Line::from(vec!["  ".into(), Span::raw("└──")]));

    lines.push("".into());
    lines.push(Line::from(vec![
      "  ".into(),
      Span::raw("To get started, describe a task or try one of these commands:"),
    ]));
    lines.push("".into());
    lines.push(Line::from(vec![
      "  ".into(),
      Span::styled("/help", ratatui::style::Style::new().bold()),
      Span::raw(" - show available commands"),
    ]));
    lines.push(Line::from(vec![
      "  ".into(),
      Span::styled("/model", ratatui::style::Style::new().bold()),
      Span::raw(" - choose model and reasoning effort"),
    ]));
    lines.push(Line::from(vec![
      "  ".into(),
      Span::styled("/status", ratatui::style::Style::new().bold()),
      Span::raw(" - show current session configuration"),
    ]));

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
  pub(crate) fn into_history_cell(ctx: WelcomeContext) -> Box<dyn HistoryCell> {
    let mut lines = Self::generate_lines(&ctx, false); // 不显示hint
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
    let lines = Self::generate_lines(&self.ctx, self.show_hint);

    // 使用Paragraph渲染，自动左对齐（与codex完全一致）
    Paragraph::new(lines)
      .wrap(Wrap { trim: false })
      .render(area, buf);
  }
}

#[cfg(test)]
mod tests {
  use super::WelcomeContext;
  use cokra_config::Config;

  #[test]
  fn welcome_context_includes_default_model() {
    let cfg = Config::default();
    let ctx = WelcomeContext::from_config(&cfg);
    assert!(ctx.model_id.contains("openai/"));
    assert!(!ctx.sandbox_mode.is_empty());
    assert!(!ctx.approval_mode.is_empty());
  }
}
