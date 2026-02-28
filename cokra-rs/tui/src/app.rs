// Cokra TUI App
// Main application state

use ratatui::Frame;

/// Main application state
pub struct App {
  /// Should exit
  pub should_quit: bool,
}

impl App {
  /// Create a new app
  pub fn new() -> Self {
    Self { should_quit: false }
  }

  /// Handle key event
  pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
    match key.code {
      crossterm::event::KeyCode::Char('q') => self.should_quit = true,
      _ => {}
    }
  }

  /// Draw the UI
  pub fn draw(&mut self, frame: &mut Frame) {
    // TODO: Implement UI rendering
    frame.render_widget(
      ratatui::widgets::Paragraph::new("Cokra TUI - Coming Soon"),
      frame.size(),
    );
  }
}

impl Default for App {
  fn default() -> Self {
    Self::new()
  }
}
