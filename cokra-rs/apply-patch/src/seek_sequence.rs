/// 1:1 codex: Attempt to find the sequence of `pattern` lines within `lines`
/// beginning at or after `start`. Returns the starting index of the match or
/// `None` if not found. Matches are attempted with decreasing strictness:
/// exact match, then ignoring trailing whitespace, then ignoring leading and
/// trailing whitespace. When `eof` is true, we first try starting at the
/// end-of-file (so that patterns intended to match file endings are applied at
/// the end), and fall back to searching from `start` if needed.
///
/// Special cases handled defensively:
///  - Empty `pattern` -> returns `Some(start)` (no-op match)
///  - `pattern.len() > lines.len()` -> returns `None` (cannot match)
pub(crate) fn seek_sequence(
  lines: &[String],
  pattern: &[String],
  start: usize,
  eof: bool,
) -> Option<usize> {
  if pattern.is_empty() {
    return Some(start);
  }

  if pattern.len() > lines.len() {
    return None;
  }
  let search_start = if eof && lines.len() >= pattern.len() {
    lines.len() - pattern.len()
  } else {
    start
  };
  // Exact match first.
  for i in search_start..=lines.len().saturating_sub(pattern.len()) {
    if lines[i..i + pattern.len()] == *pattern {
      return Some(i);
    }
  }
  // Then rstrip match.
  for i in search_start..=lines.len().saturating_sub(pattern.len()) {
    let mut ok = true;
    for (p_idx, pat) in pattern.iter().enumerate() {
      if lines[i + p_idx].trim_end() != pat.trim_end() {
        ok = false;
        break;
      }
    }
    if ok {
      return Some(i);
    }
  }
  // Finally, trim both sides to allow more lenience.
  for i in search_start..=lines.len().saturating_sub(pattern.len()) {
    let mut ok = true;
    for (p_idx, pat) in pattern.iter().enumerate() {
      if lines[i + p_idx].trim() != pat.trim() {
        ok = false;
        break;
      }
    }
    if ok {
      return Some(i);
    }
  }

  // Final, most permissive pass – normalise common Unicode punctuation to
  // ASCII equivalents so that diffs authored with plain ASCII characters can
  // still be applied to source files that contain typographic dashes/quotes.
  fn normalise(s: &str) -> String {
    s.trim()
      .chars()
      .map(|c| match c {
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
        | '\u{2212}' => '-',
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
        '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
        | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
        | '\u{3000}' => ' ',
        other => other,
      })
      .collect::<String>()
  }

  for i in search_start..=lines.len().saturating_sub(pattern.len()) {
    let mut ok = true;
    for (p_idx, pat) in pattern.iter().enumerate() {
      if normalise(&lines[i + p_idx]) != normalise(pat) {
        ok = false;
        break;
      }
    }
    if ok {
      return Some(i);
    }
  }

  None
}

#[cfg(test)]
mod tests {
  use super::seek_sequence;
  use std::string::ToString;

  fn to_vec(strings: &[&str]) -> Vec<String> {
    strings.iter().map(ToString::to_string).collect()
  }

  #[test]
  fn test_exact_match_finds_sequence() {
    let lines = to_vec(&["foo", "bar", "baz"]);
    let pattern = to_vec(&["bar", "baz"]);
    assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(1));
  }

  #[test]
  fn test_rstrip_match_ignores_trailing_whitespace() {
    let lines = to_vec(&["foo   ", "bar\t\t"]);
    let pattern = to_vec(&["foo", "bar"]);
    assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
  }

  #[test]
  fn test_trim_match_ignores_leading_and_trailing_whitespace() {
    let lines = to_vec(&["    foo   ", "   bar\t"]);
    let pattern = to_vec(&["foo", "bar"]);
    assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
  }

  #[test]
  fn test_pattern_longer_than_input_returns_none() {
    let lines = to_vec(&["just one line"]);
    let pattern = to_vec(&["too", "many", "lines"]);
    assert_eq!(seek_sequence(&lines, &pattern, 0, false), None);
  }

  #[test]
  fn test_empty_pattern_returns_start() {
    let lines = to_vec(&["foo", "bar"]);
    let pattern: Vec<String> = vec![];
    assert_eq!(seek_sequence(&lines, &pattern, 1, false), Some(1));
  }

  #[test]
  fn test_eof_searches_from_end() {
    let lines = to_vec(&["a", "b", "c", "b", "c"]);
    let pattern = to_vec(&["b", "c"]);
    assert_eq!(seek_sequence(&lines, &pattern, 0, true), Some(3));
  }

  #[test]
  fn test_unicode_normalisation() {
    // EN DASH in file, ASCII dash in pattern
    let lines = vec!["import asyncio \u{2013} foo".to_string()];
    let pattern = vec!["import asyncio - foo".to_string()];
    assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
  }
}
