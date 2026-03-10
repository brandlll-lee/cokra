// Cokra TUI
// Terminal UI implementation

use anyhow::Result;
use cokra_core::Cokra;

pub mod app;
pub mod custom_terminal;
pub mod insert_history;
pub mod welcome;
pub mod widgets;

pub(crate) mod app_event;
pub(crate) mod app_event_sender;
pub(crate) mod bottom_pane;
pub(crate) mod chatwidget;
pub(crate) mod color;
pub(crate) mod exec_cell;
pub(crate) mod exec_command;
pub(crate) mod history_cell;
pub(crate) mod key_hint;
pub(crate) mod markdown;
pub(crate) mod markdown_render;
pub(crate) mod markdown_stream;
pub(crate) mod multi_agents;
pub(crate) mod path_utils;
pub(crate) mod render;
pub(crate) mod shimmer;
pub(crate) mod slash_command;
pub(crate) mod status_indicator_widget;
pub(crate) mod streaming;
pub(crate) mod style;
pub(crate) mod terminal_palette;
pub(crate) mod text_formatting;
pub(crate) mod tui;
pub(crate) mod ui_consts;
pub(crate) mod wrapping;
pub(crate) mod xml_filter;

pub use app::App;
pub use app::AppExitInfo;
pub use app::ExitReason;
pub use app_event::UiMode;

/// Run the full-screen TUI application.
pub async fn run_main(cokra: Cokra, ui_mode: UiMode) -> Result<AppExitInfo> {
  let terminal = tui::init()?;
  let mut tui = tui::Tui::new(terminal);

  match ui_mode {
    UiMode::AltScreen => tui.enter_alt_screen()?,
    UiMode::Inline => {}
  }

  let frame_requester = tui.frame_requester();
  let mut app = App::new(cokra, frame_requester, ui_mode);

  let result = app.run(&mut tui).await;

  if ui_mode == UiMode::AltScreen {
    let _ = tui.leave_alt_screen();
  }
  let restore_result = tui::restore();

  match (result, restore_result) {
    (Ok(exit_info), Ok(())) => Ok(exit_info),
    (Err(run_err), Ok(())) => Err(run_err),
    (Ok(_), Err(restore_err)) => Err(restore_err.into()),
    (Err(run_err), Err(_restore_err)) => Err(run_err),
  }
}
