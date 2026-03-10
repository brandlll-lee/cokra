//! 1:1 codex BottomPaneView trait.
//!
//! Every view that can be shown in the bottom pane's view stack must implement
//! this trait. When the view stack is non-empty, the topmost view **replaces**
//! the composer in rendering and key handling.

use crossterm::event::KeyEvent;

use crate::render::renderable::Renderable;
use crate::tui::InlineViewportSizing;

/// 1:1 codex: trait implemented by every view shown in the bottom pane.
pub(crate) trait BottomPaneView: Renderable {
  /// Controls how inline-mode viewport sizing behaves while this view is active.
  ///
  /// Dialog-style views should preserve visible history so resize redraws do not
  /// push the dialog itself into scrollback. Larger list-style overlays may opt
  /// into overlay expansion explicitly.
  fn inline_viewport_sizing(&self) -> InlineViewportSizing {
    InlineViewportSizing::PreserveVisibleHistory
  }

  /// Handle a key event while the view is active.
  fn handle_key_event(&mut self, _key_event: KeyEvent) {}

  /// Handle paste while this view is active. Return true if the view updated
  /// its state and needs a redraw.
  fn handle_paste(&mut self, _text: String) -> bool {
    false
  }

  /// Return `true` if the view has finished and should be removed.
  fn is_complete(&self) -> bool {
    false
  }

  /// Handle Esc / Ctrl-C while this view is active.
  /// Return `true` if the cancellation was consumed (view will be popped).
  fn on_cancel(&mut self) -> bool {
    true
  }
}
