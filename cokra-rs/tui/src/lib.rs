// Cokra TUI
// Terminal UI implementation

use std::time::Duration;

use anyhow::Result;
use cokra_core::Cokra;

pub mod app;
pub mod widgets;

pub(crate) mod app_event;
pub(crate) mod app_event_sender;
pub(crate) mod bottom_pane;
pub(crate) mod chatwidget;
pub(crate) mod color;
pub(crate) mod exec_cell;
pub(crate) mod history_cell;
pub(crate) mod key_hint;
pub(crate) mod markdown;
pub(crate) mod markdown_render;
pub(crate) mod markdown_stream;
pub(crate) mod multi_agents;
pub(crate) mod render;
pub(crate) mod shimmer;
pub(crate) mod status_indicator_widget;
pub(crate) mod streaming;
pub(crate) mod style;
pub(crate) mod terminal_palette;
pub(crate) mod text_formatting;
pub(crate) mod tui;
pub(crate) mod wrapping;

pub use app::App;
pub use app::AppExitInfo;
pub use app::ExitReason;

/// Run the full-screen TUI application.
pub async fn run_main(cokra: Cokra) -> Result<AppExitInfo> {
  tui::set_modes()?;
  let mut terminal = tui::init_terminal()?;

  let (draw_tx, frame_requester) = app::make_frame_requester();
  let mut events = tui::TuiEventStream::new(draw_tx.subscribe(), Duration::from_millis(32));
  let mut app = App::new(cokra, frame_requester);

  let result = app.run(&mut terminal, &mut events).await;
  let restore_result = tui::restore_modes();

  match (result, restore_result) {
    (Ok(exit_info), Ok(())) => Ok(exit_info),
    (Err(run_err), Ok(())) => Err(run_err),
    (Ok(_), Err(restore_err)) => Err(restore_err.into()),
    (Err(run_err), Err(_restore_err)) => Err(run_err),
  }
}
