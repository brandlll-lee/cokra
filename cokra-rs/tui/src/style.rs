use crate::color::blend;
use crate::color::is_light;
use crate::color::perceptual_distance;
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
  let (r, g, b) = terminal_bg;
  let luma = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0;

  let (top, mut alpha, alpha_max, min_distance) = if is_light(terminal_bg) {
    // Darken slightly on light themes. Scale up a bit for very bright terminals so the
    // bar remains visible without looking heavy.
    let alpha = (0.08 + (luma - 0.55).max(0.0) * 0.044).clamp(0.08, 0.10);
    ((0, 0, 0), alpha, 0.16, 6.0)
  } else {
    // Lighten more aggressively on dark themes to match Claude Code-style message bars.
    // Dimmer terminals get a slightly stronger tint.
    let alpha = (0.24 - luma * 0.08).clamp(0.20, 0.24);
    ((255, 255, 255), alpha, 0.30, 8.0)
  };

  let mut blended = blend(top, terminal_bg, alpha);
  while perceptual_distance(blended, terminal_bg) < min_distance && alpha < alpha_max {
    alpha = (alpha + 0.02).min(alpha_max);
    blended = blend(top, terminal_bg, alpha);
  }

  best_color(blended)
}

#[allow(clippy::disallowed_methods)]
pub fn proposed_plan_bg(terminal_bg: (u8, u8, u8)) -> Color {
  user_message_bg(terminal_bg)
}
