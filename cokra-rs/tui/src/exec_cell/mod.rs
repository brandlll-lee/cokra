use std::time::Instant;

use ratatui::style::Stylize;
use ratatui::text::Span;

use crate::shimmer::shimmer_spans;

pub(crate) mod model;
pub(crate) mod render;

pub(crate) use model::ExecCall;
pub(crate) use model::ExecCell;
pub(crate) use render::new_active_exec_command;

pub(crate) fn spinner(start_time: Option<Instant>, animations_enabled: bool) -> Span<'static> {
  if !animations_enabled {
    return "•".dim();
  }
  let elapsed = start_time.map(|st| st.elapsed()).unwrap_or_default();
  if supports_color::on_cached(supports_color::Stream::Stdout)
    .map(|level| level.has_16m)
    .unwrap_or(false)
  {
    shimmer_spans("•")[0].clone()
  } else {
    let blink_on = (elapsed.as_millis() / 600).is_multiple_of(2);
    if blink_on { "•".into() } else { "◦".dim() }
  }
}
