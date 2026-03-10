use std::fmt;
use std::future::Future;
use std::io::IsTerminal;
use std::io::Result;
use std::io::Stdout;
use std::io::stdin;
use std::io::stdout;
use std::panic;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crossterm::Command;
use crossterm::SynchronizedUpdate;
use crossterm::event::DisableBracketedPaste;
use crossterm::event::DisableFocusChange;
use crossterm::event::EnableBracketedPaste;
use crossterm::event::EnableFocusChange;
use crossterm::event::KeyEvent;
use crossterm::event::KeyboardEnhancementFlags;
use crossterm::event::PopKeyboardEnhancementFlags;
use crossterm::event::PushKeyboardEnhancementFlags;
use crossterm::execute;
use crossterm::terminal::EnterAlternateScreen;
use crossterm::terminal::LeaveAlternateScreen;
use crossterm::terminal::disable_raw_mode;
use crossterm::terminal::enable_raw_mode;
use crossterm::terminal::supports_keyboard_enhancement;
use ratatui::backend::CrosstermBackend;
use ratatui::backend::Backend;
use ratatui::layout::Offset;
use ratatui::layout::Rect;
use ratatui::text::Line;
use tokio::sync::broadcast;
use tokio_stream::Stream;

pub use self::frame_requester::FrameRequester;
use crate::custom_terminal;
use crate::custom_terminal::Terminal as CustomTerminal;
use crate::tui::event_stream::EventBroker;
use crate::tui::event_stream::TuiEventStream;
#[cfg(unix)]
use crate::tui::job_control::SuspendContext;

mod event_stream;
mod frame_rate_limiter;
mod frame_requester;
#[cfg(unix)]
mod job_control;

/// Target frame interval for UI redraw scheduling.
pub(crate) const TARGET_FRAME_INTERVAL: Duration = frame_rate_limiter::MIN_FRAME_INTERVAL;

/// A type alias for the terminal type used in this application
pub type Terminal = CustomTerminal<CrosstermBackend<Stdout>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InlineViewportSizing {
  PreserveVisibleHistory,
  ExpandForOverlay,
}

fn inline_viewport_height(
  requested_height: u16,
  screen_height: u16,
  visible_history_rows: u16,
  sizing: InlineViewportSizing,
) -> u16 {
  if screen_height == 0 {
    return 0;
  }

  if sizing == InlineViewportSizing::ExpandForOverlay {
    return requested_height.min(screen_height);
  }

  // Inline mode relies on `insert_history_lines()` to render committed history *above* the viewport.
  // That implementation uses a scroll region capped at `viewport.top()`, which becomes invalid when
  // the viewport reaches the very top of the screen (top == 0).
  //
  // Keep at least one row above the viewport whenever possible so:
  // - history insertions never need to emit `ESC[1;0r` (broken scroll region)
  // - the compositor never "covers" freshly inserted history, which looks like the user message
  //   was swallowed
  //
  // Tradeoff: even when no history has been inserted yet, the inline viewport is capped to
  // `screen_height - 1` on screens that are at least 2 rows tall.
  let reserved_history_rows = if screen_height > 1 {
    visible_history_rows.max(1).min(screen_height.saturating_sub(1))
  } else {
    0
  };
  // Tradeoff: preserve already visible history above the inline viewport even if that means
  // the live transcript must scroll within a smaller viewport once the lower region fills up.
  let max_inline_height = screen_height.saturating_sub(reserved_history_rows).max(1);
  requested_height.min(max_inline_height)
}

pub fn set_modes() -> Result<()> {
  execute!(stdout(), EnableBracketedPaste)?;

  enable_raw_mode()?;
  // Enable keyboard enhancement flags so modifiers for keys like Enter are disambiguated.
  // Some terminals (notably legacy Windows consoles) do not support
  // keyboard enhancement flags. Attempt to enable them, but continue
  // gracefully if unsupported.
  let _ = execute!(
    stdout(),
    PushKeyboardEnhancementFlags(
      KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
    )
  );

  let _ = execute!(stdout(), EnableFocusChange);
  Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EnableAlternateScroll;

impl Command for EnableAlternateScroll {
  fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
    write!(f, "\x1b[?1007h")
  }

  #[cfg(windows)]
  fn execute_winapi(&self) -> Result<()> {
    Err(std::io::Error::other(
      "tried to execute EnableAlternateScroll using WinAPI; use ANSI instead",
    ))
  }

  #[cfg(windows)]
  fn is_ansi_code_supported(&self) -> bool {
    true
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DisableAlternateScroll;

impl Command for DisableAlternateScroll {
  fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
    write!(f, "\x1b[?1007l")
  }

  #[cfg(windows)]
  fn execute_winapi(&self) -> Result<()> {
    Err(std::io::Error::other(
      "tried to execute DisableAlternateScroll using WinAPI; use ANSI instead",
    ))
  }

  #[cfg(windows)]
  fn is_ansi_code_supported(&self) -> bool {
    true
  }
}

fn restore_common(should_disable_raw_mode: bool) -> Result<()> {
  // Pop may fail on platforms that didn't support the push; ignore errors.
  let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
  execute!(stdout(), DisableBracketedPaste)?;
  let _ = execute!(stdout(), DisableFocusChange);
  if should_disable_raw_mode {
    disable_raw_mode()?;
  }
  let _ = execute!(stdout(), crossterm::cursor::Show);
  Ok(())
}

/// Restore the terminal to its original state.
/// Inverse of `set_modes`.
pub fn restore() -> Result<()> {
  let should_disable_raw_mode = true;
  restore_common(should_disable_raw_mode)
}

/// Restore the terminal to its original state, but keep raw mode enabled.
pub fn restore_keep_raw() -> Result<()> {
  let should_disable_raw_mode = false;
  restore_common(should_disable_raw_mode)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreMode {
  #[allow(dead_code)]
  Full, // Fully restore the terminal (disables raw mode).
  KeepRaw, // Restore the terminal but keep raw mode enabled.
}

impl RestoreMode {
  fn restore(self) -> Result<()> {
    match self {
      RestoreMode::Full => restore(),
      RestoreMode::KeepRaw => restore_keep_raw(),
    }
  }
}

/// Flush the underlying stdin buffer to clear any input that may be buffered at the terminal level.
#[cfg(unix)]
fn flush_terminal_input_buffer() {
  // Safety: flushing the stdin queue is safe and does not move ownership.
  let result = unsafe { libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH) };
  if result != 0 {
    let err = std::io::Error::last_os_error();
    tracing::warn!("failed to tcflush stdin: {err}");
  }
}

#[cfg(windows)]
fn flush_terminal_input_buffer() {
  use windows_sys::Win32::Foundation::GetLastError;
  use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
  use windows_sys::Win32::System::Console::FlushConsoleInputBuffer;
  use windows_sys::Win32::System::Console::GetStdHandle;
  use windows_sys::Win32::System::Console::STD_INPUT_HANDLE;

  let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
  if handle == INVALID_HANDLE_VALUE || handle == 0 {
    let err = unsafe { GetLastError() };
    tracing::warn!("failed to get stdin handle for flush: error {err}");
    return;
  }

  let result = unsafe { FlushConsoleInputBuffer(handle) };
  if result == 0 {
    let err = unsafe { GetLastError() };
    tracing::warn!("failed to flush stdin buffer: error {err}");
  }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn flush_terminal_input_buffer() {}

/// Initialize the terminal (inline viewport; history stays in normal scrollback)
pub fn init() -> Result<Terminal> {
  if !stdin().is_terminal() {
    return Err(std::io::Error::other("stdin is not a terminal"));
  }
  if !stdout().is_terminal() {
    return Err(std::io::Error::other("stdout is not a terminal"));
  }
  set_modes()?;

  flush_terminal_input_buffer();

  set_panic_hook();

  let backend = CrosstermBackend::new(stdout());
  let tui = CustomTerminal::with_options(backend)?;
  Ok(tui)
}

fn set_panic_hook() {
  let hook = panic::take_hook();
  panic::set_hook(Box::new(move |panic_info| {
    let _ = restore(); // ignore any errors as we are already failing
    hook(panic_info);
  }));
}

#[derive(Clone, Debug)]
pub enum TuiEvent {
  Key(KeyEvent),
  Paste(String),
  Draw,
}

pub struct Tui {
  frame_requester: FrameRequester,
  draw_tx: broadcast::Sender<()>,
  event_broker: Arc<EventBroker>,
  pub(crate) terminal: Terminal,
  pending_history_lines: Vec<Line<'static>>,
  alt_saved_viewport: Option<ratatui::layout::Rect>,
  #[cfg(unix)]
  suspend_context: SuspendContext,
  // True when overlay alt-screen UI is active
  alt_screen_active: Arc<AtomicBool>,
  // True when terminal/tab is focused; updated internally from crossterm events
  terminal_focused: Arc<AtomicBool>,
  enhanced_keys_supported: bool,
  // When false, enter_alt_screen() becomes a no-op (for Zellij scrollback support)
  alt_screen_enabled: bool,
}

impl Tui {
  pub fn new(terminal: Terminal) -> Self {
    let (draw_tx, _) = broadcast::channel(1);
    let frame_requester = FrameRequester::new(draw_tx.clone());

    // Detect keyboard enhancement support before any EventStream is created so the
    // crossterm poller can acquire its lock without contention.
    let enhanced_keys_supported = supports_keyboard_enhancement().unwrap_or(false);
    // Cache this to avoid contention with the event reader.
    supports_color::on_cached(supports_color::Stream::Stdout);
    let _ = crate::terminal_palette::default_colors();

    Self {
      frame_requester,
      draw_tx,
      event_broker: Arc::new(EventBroker::new()),
      terminal,
      pending_history_lines: vec![],
      alt_saved_viewport: None,
      #[cfg(unix)]
      suspend_context: SuspendContext::new(),
      alt_screen_active: Arc::new(AtomicBool::new(false)),
      terminal_focused: Arc::new(AtomicBool::new(true)),
      enhanced_keys_supported,
      alt_screen_enabled: true,
    }
  }

  /// Set whether alternate screen is enabled. When false, enter_alt_screen() becomes a no-op.
  pub fn set_alt_screen_enabled(&mut self, enabled: bool) {
    self.alt_screen_enabled = enabled;
  }

  pub fn frame_requester(&self) -> FrameRequester {
    self.frame_requester.clone()
  }

  pub fn enhanced_keys_supported(&self) -> bool {
    self.enhanced_keys_supported
  }

  pub fn is_alt_screen_active(&self) -> bool {
    self.alt_screen_active.load(Ordering::Relaxed)
  }

  // Drop crossterm EventStream to avoid stdin conflicts with other processes.
  pub fn pause_events(&mut self) {
    self.event_broker.pause_events();
  }

  // Resume crossterm EventStream to resume stdin polling.
  // Inverse of `pause_events`.
  pub fn resume_events(&mut self) {
    self.event_broker.resume_events();
  }

  /// Temporarily restore terminal state to run an external interactive program `f`.
  pub async fn with_restored<R, F, Fut>(&mut self, mode: RestoreMode, f: F) -> R
  where
    F: FnOnce() -> Fut,
    Fut: Future<Output = R>,
  {
    // Pause crossterm events to avoid stdin conflicts with external program `f`.
    self.pause_events();

    // Leave alt screen if active to avoid conflicts with external program `f`.
    let was_alt_screen = self.is_alt_screen_active();
    if was_alt_screen {
      let _ = self.leave_alt_screen();
    }

    if let Err(err) = mode.restore() {
      tracing::warn!("failed to restore terminal modes before external program: {err}");
    }

    let output = f().await;

    if let Err(err) = set_modes() {
      tracing::warn!("failed to re-enable terminal modes after external program: {err}");
    }
    // After the external program `f` finishes, reset terminal state and flush any buffered keypresses.
    flush_terminal_input_buffer();

    if was_alt_screen {
      let _ = self.enter_alt_screen();
    }

    self.resume_events();
    output
  }

  pub fn event_stream(&self) -> Pin<Box<dyn Stream<Item = TuiEvent> + Send + 'static>> {
    #[cfg(unix)]
    let stream = TuiEventStream::new(
      self.event_broker.clone(),
      self.draw_tx.subscribe(),
      self.terminal_focused.clone(),
      self.suspend_context.clone(),
      self.alt_screen_active.clone(),
    );
    #[cfg(not(unix))]
    let stream = TuiEventStream::new(
      self.event_broker.clone(),
      self.draw_tx.subscribe(),
      self.terminal_focused.clone(),
    );
    Box::pin(stream)
  }

  /// Enter alternate screen and expand the viewport to full terminal size, saving the current
  /// inline viewport for restoration when leaving.
  pub fn enter_alt_screen(&mut self) -> Result<()> {
    if !self.alt_screen_enabled {
      return Ok(());
    }
    let _ = execute!(self.terminal.backend_mut(), EnterAlternateScreen);
    // Enable "alternate scroll" so terminals may translate wheel to arrows
    let _ = execute!(self.terminal.backend_mut(), EnableAlternateScroll);
    if let Ok(size) = self.terminal.size() {
      self.alt_saved_viewport = Some(self.terminal.viewport_area);
      self
        .terminal
        .set_viewport_area(ratatui::layout::Rect::new(0, 0, size.width, size.height));
      let _ = self.terminal.clear();
    }
    self.alt_screen_active.store(true, Ordering::Relaxed);
    Ok(())
  }

  /// Leave alternate screen and restore the previously saved inline viewport, if any.
  pub fn leave_alt_screen(&mut self) -> Result<()> {
    if !self.alt_screen_enabled {
      return Ok(());
    }
    // Disable alternate scroll when leaving alt-screen
    let _ = execute!(self.terminal.backend_mut(), DisableAlternateScroll);
    let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
    if let Some(saved) = self.alt_saved_viewport.take() {
      self.terminal.set_viewport_area(saved);
    }
    self.alt_screen_active.store(false, Ordering::Relaxed);
    Ok(())
  }

  pub fn insert_history_lines(&mut self, lines: Vec<Line<'static>>) {
    self.pending_history_lines.extend(lines);
    self.frame_requester().schedule_frame();
  }

  pub fn clear_pending_history_lines(&mut self) {
    self.pending_history_lines.clear();
  }

  pub fn draw(
    &mut self,
    height: u16,
    sizing: InlineViewportSizing,
    draw_fn: impl FnOnce(&mut custom_terminal::Frame),
  ) -> Result<()> {
    // If we are resuming from ^Z, we need to prepare the resume action now so we can apply it
    // in the synchronized update.
    #[cfg(unix)]
    let mut prepared_resume = self
      .suspend_context
      .prepare_resume_action(&mut self.terminal, &mut self.alt_saved_viewport);

    // Precompute any viewport updates that need a cursor-position query before entering
    // the synchronized update, to avoid racing with the event reader.
    let mut pending_viewport_area = self.pending_viewport_area()?;

    stdout().sync_update(|_| {
      #[cfg(unix)]
      if let Some(prepared) = prepared_resume.take() {
        prepared.apply(&mut self.terminal)?;
      }

      let terminal = &mut self.terminal;
      if let Some(new_area) = pending_viewport_area.take() {
        terminal.set_viewport_area(new_area);
        terminal.clear()?;
      }

      let size = terminal.size()?;

      let mut area = terminal.viewport_area;
      area.height =
        inline_viewport_height(height, size.height, terminal.visible_history_rows(), sizing);
      area.width = size.width;
      if !self.alt_screen_active.load(Ordering::Relaxed) && size.height > 1 {
        // In inline mode, keep at least one row above the viewport. This avoids invalid scroll
        // regions during `insert_history_lines()` and prevents "history swallowed by viewport"
        // when the screen is resized small.
        //
        // Tradeoff: overlays that request full-screen height in inline mode will be clipped by 1 row.
        area.height = area.height.min(size.height.saturating_sub(1)).max(1);
        area.y = area.y.max(1);
      }
      if area.bottom() > size.height {
        // Inline mode runs inside the normal terminal screen buffer.
        // If the viewport expanded, scroll the region *above* it up to make room.
        //
        // This matches codex's proven behavior and ensures that freshly inserted scrollback lines
        // remain visible instead of being covered by a bottom-anchored viewport.
        terminal
          .backend_mut()
          .scroll_region_up(0..area.top(), area.bottom() - size.height)?;
        area.y = size.height.saturating_sub(area.height);
      }
      if area != terminal.viewport_area {
        terminal.clear()?;
        terminal.set_viewport_area(area);
      }

      if !self.pending_history_lines.is_empty() {
        crate::insert_history::insert_history_lines(terminal, self.pending_history_lines.clone())?;
        self.pending_history_lines.clear();
      }

      // Update the y position for suspending so Ctrl-Z can place the cursor correctly.
      #[cfg(unix)]
      {
        let inline_area_bottom = if self.alt_screen_active.load(Ordering::Relaxed) {
          self
            .alt_saved_viewport
            .map(|r| r.bottom().saturating_sub(1))
            .unwrap_or_else(|| area.bottom().saturating_sub(1))
        } else {
          area.bottom().saturating_sub(1)
        };
        self.suspend_context.set_cursor_y(inline_area_bottom);
      }

      terminal.draw(|frame| {
        draw_fn(frame);
      })
    })?
  }

  fn pending_viewport_area(&mut self) -> Result<Option<Rect>> {
    let terminal = &mut self.terminal;
    let screen_size = terminal.size()?;
    let last_known_screen_size = terminal.last_known_screen_size;
    if screen_size != last_known_screen_size
      && let Ok(cursor_pos) = terminal.get_cursor_position()
    {
      let last_known_cursor_pos = terminal.last_known_cursor_pos;
      // If we resized AND the cursor moved, we adjust the viewport area to keep the
      // cursor in the same position.
      if cursor_pos.y != last_known_cursor_pos.y {
        let offset = Offset {
          x: 0,
          y: cursor_pos.y as i32 - last_known_cursor_pos.y as i32,
        };
        return Ok(Some(terminal.viewport_area.offset(offset)));
      }
    }
    Ok(None)
  }
}

#[cfg(test)]
mod tests {
  use super::InlineViewportSizing;
  use super::inline_viewport_height;

  #[test]
  fn cap_inline_viewport_height_preserves_visible_history_rows() {
    assert_eq!(
      12,
      inline_viewport_height(12, 24, 8, InlineViewportSizing::PreserveVisibleHistory)
    );
    assert_eq!(
      16,
      inline_viewport_height(20, 24, 8, InlineViewportSizing::PreserveVisibleHistory)
    );
  }

  #[test]
  fn cap_inline_viewport_height_keeps_one_row_for_inline_ui() {
    assert_eq!(
      1,
      inline_viewport_height(5, 6, 6, InlineViewportSizing::PreserveVisibleHistory)
    );
    assert_eq!(
      0,
      inline_viewport_height(5, 0, 0, InlineViewportSizing::PreserveVisibleHistory)
    );
  }

  #[test]
  fn overlay_mode_matches_codex_style_full_expansion() {
    assert_eq!(
      20,
      inline_viewport_height(20, 24, 8, InlineViewportSizing::ExpandForOverlay)
    );
    assert_eq!(
      24,
      inline_viewport_height(40, 24, 8, InlineViewportSizing::ExpandForOverlay)
    );
  }

  #[test]
  fn preserve_visible_history_always_leaves_one_row_when_possible() {
    // Even with no already-inserted scrollback, we reserve one row above the
    // inline viewport so insert_history_lines has a valid region.
    assert_eq!(
      23,
      inline_viewport_height(40, 24, 0, InlineViewportSizing::PreserveVisibleHistory)
    );
    assert_eq!(
      1,
      inline_viewport_height(40, 2, 0, InlineViewportSizing::PreserveVisibleHistory)
    );
    // Degenerate 1-row terminals can't reserve space.
    assert_eq!(
      1,
      inline_viewport_height(40, 1, 0, InlineViewportSizing::PreserveVisibleHistory)
    );
  }
}
