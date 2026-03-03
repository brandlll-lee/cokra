use std::io;
use std::io::Stdout;

use crossterm::event::DisableBracketedPaste;
use crossterm::event::EnableBracketedPaste;
use crossterm::event::KeyboardEnhancementFlags;
use crossterm::event::PopKeyboardEnhancementFlags;
use crossterm::event::PushKeyboardEnhancementFlags;
use crossterm::execute;
use crossterm::terminal::EnterAlternateScreen;
use crossterm::terminal::LeaveAlternateScreen;
use crossterm::terminal::disable_raw_mode;
use crossterm::terminal::enable_raw_mode;
use ratatui::Terminal as RatatuiTerminal;
use ratatui::backend::CrosstermBackend;

mod event_stream;
mod frame_rate_limiter;
mod frame_requester;
#[cfg(unix)]
mod job_control;

pub(crate) use event_stream::TuiEvent;
pub(crate) use event_stream::TuiEventStream;
pub use frame_requester::FrameRequester;

pub type Terminal = RatatuiTerminal<CrosstermBackend<Stdout>>;

pub(crate) fn set_modes() -> io::Result<()> {
  let mut out = io::stdout();
  execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
  enable_raw_mode()?;
  let _ = execute!(
    out,
    PushKeyboardEnhancementFlags(
      KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
    )
  );
  Ok(())
}

pub(crate) fn restore_modes() -> io::Result<()> {
  let mut out = io::stdout();
  let _ = execute!(out, PopKeyboardEnhancementFlags);
  let _ = disable_raw_mode();
  execute!(out, DisableBracketedPaste, LeaveAlternateScreen)?;
  Ok(())
}

pub(crate) fn init_terminal() -> io::Result<Terminal> {
  let backend = CrosstermBackend::new(io::stdout());
  RatatuiTerminal::new(backend)
}
