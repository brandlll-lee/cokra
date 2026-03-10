use crate::color::blend;
use crate::color::is_light;
use crate::terminal_palette::best_color;
use crate::terminal_palette::default_bg;
use ratatui::style::Color;
use ratatui::style::Style;

pub fn user_message_style() -> Style {
  user_message_style_for(default_bg())
}

pub fn proposed_plan_style() -> Style {
  proposed_plan_style_for(default_bg())
}

/// Returns the style for a user-authored message using the provided terminal background.
pub fn user_message_style_for(terminal_bg: Option<(u8, u8, u8)>) -> Style {
  match terminal_bg {
    Some(bg) => Style::default().bg(user_message_bg(bg)),
    None => {
      // When the terminal background cannot be detected, still provide a stable
      // filled surface so user messages render as Claude Code-style bars instead
      // of bordered boxes.
      //
      // Tradeoff: on uncommon light-themed terminals without COLORFGBG (or similar),
      // this fallback may appear stronger than intended.
      Style::default().bg(best_color((48, 48, 48)))
    }
  }
}

pub fn proposed_plan_style_for(terminal_bg: Option<(u8, u8, u8)>) -> Style {
  match terminal_bg {
    Some(bg) => Style::default().bg(proposed_plan_bg(bg)),
    None => Style::default().bg(best_color((48, 48, 48))),
  }
}

#[allow(clippy::disallowed_methods)]
pub fn user_message_bg(terminal_bg: (u8, u8, u8)) -> Color {
  let (top, alpha) = if is_light(terminal_bg) {
    ((0, 0, 0), 0.04)
  } else {
    ((255, 255, 255), 0.12)
  };
  best_color(blend(top, terminal_bg, alpha))
}

#[allow(clippy::disallowed_methods)]
pub fn proposed_plan_bg(terminal_bg: (u8, u8, u8)) -> Color {
  user_message_bg(terminal_bg)
}
