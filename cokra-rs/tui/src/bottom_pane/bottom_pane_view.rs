//! 1:1 codex BottomPaneView trait.
//!
//! Every view that can be shown in the bottom pane's view stack must implement
//! this trait. When the view stack is non-empty, the topmost view **replaces**
//! the composer in rendering and key handling.

use crossterm::event::KeyEvent;

use crate::render::renderable::Renderable;

/// 1:1 codex: trait implemented by every view shown in the bottom pane.
pub(crate) trait BottomPaneView: Renderable {
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
