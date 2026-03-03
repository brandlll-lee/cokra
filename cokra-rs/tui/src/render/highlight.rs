use ratatui::text::Line;

/// Temporary highlight stub for phase A.
///
/// We intentionally keep this minimal until full syntax-highlighting migration.
#[allow(dead_code)]
pub(crate) fn highlight_bash_to_lines(script: &str) -> Vec<Line<'static>> {
  script
    .lines()
    .map(|line| Line::from(line.to_string()))
    .collect()
}
