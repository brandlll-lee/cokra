use std::time::Instant;

use ratatui::style::Stylize;
use ratatui::text::Span;

pub(crate) mod model;
pub(crate) mod render;

pub(crate) use model::ExecCall;
pub(crate) use model::ExecCell;
pub(crate) use render::new_active_exec_command;

/// Braille spinner frames matching opencode's spinner component.
const BRAILLE_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Interval between braille frames (80ms, matching opencode).
const SPINNER_INTERVAL_MS: u128 = 80;

pub(crate) fn spinner(start_time: Option<Instant>, animations_enabled: bool) -> Span<'static> {
  if !animations_enabled {
    return "⋯".dim();
  }
  let elapsed_ms = start_time.map(|st| st.elapsed().as_millis()).unwrap_or(0);
  let idx = (elapsed_ms / SPINNER_INTERVAL_MS) as usize % BRAILLE_FRAMES.len();
  BRAILLE_FRAMES[idx].into()
}
