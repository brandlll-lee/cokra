use std::borrow::Cow;

/// Truncation strategy for model-facing text payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TruncationPolicy {
  /// Keep up to N lines.
  Lines(usize),
  /// Keep up to N estimated tokens.
  Tokens(usize),
  /// Do not truncate.
  #[default]
  None,
}

/// Phase-0 default budget for tool output sent back to model.
pub const DEFAULT_TOOL_OUTPUT_TOKENS: usize = 16_384;

impl std::ops::Mul<f64> for TruncationPolicy {
  type Output = Self;

  fn mul(self, multiplier: f64) -> Self::Output {
    match self {
      TruncationPolicy::Lines(lines) => {
        TruncationPolicy::Lines((lines as f64 * multiplier).ceil() as usize)
      }
      TruncationPolicy::Tokens(tokens) => {
        TruncationPolicy::Tokens((tokens as f64 * multiplier).ceil() as usize)
      }
      TruncationPolicy::None => TruncationPolicy::None,
    }
  }
}

/// Truncate text and include omission marker suitable for model consumption.
pub fn formatted_truncate_text(text: &str, policy: TruncationPolicy) -> String {
  match policy {
    TruncationPolicy::None => text.to_string(),
    TruncationPolicy::Lines(max_lines) => truncate_lines(text, max_lines, true),
    TruncationPolicy::Tokens(max_tokens) => truncate_tokens(text, max_tokens, true),
  }
}

/// Truncate text without additional formatting guarantees.
pub fn truncate_text(text: &str, policy: TruncationPolicy) -> String {
  match policy {
    TruncationPolicy::None => text.to_string(),
    TruncationPolicy::Lines(max_lines) => truncate_lines(text, max_lines, false),
    TruncationPolicy::Tokens(max_tokens) => truncate_tokens(text, max_tokens, false),
  }
}

fn truncate_lines(text: &str, max_lines: usize, include_marker: bool) -> String {
  if max_lines == 0 {
    return String::new();
  }

  let lines: Vec<&str> = text.lines().collect();
  if lines.len() <= max_lines {
    return text.to_string();
  }

  let head = max_lines / 2;
  let tail = max_lines.saturating_sub(head);
  let omitted = lines.len().saturating_sub(head + tail);

  let mut out = String::new();
  out.push_str(&lines[..head].join("\n"));
  if include_marker {
    if !out.is_empty() {
      out.push('\n');
    }
    out.push_str(&format!("... ({} lines omitted) ...", omitted));
  }
  if !lines[lines.len() - tail..].is_empty() {
    if !out.is_empty() {
      out.push('\n');
    }
    out.push_str(&lines[lines.len() - tail..].join("\n"));
  }
  if text.ends_with('\n') && !out.ends_with('\n') {
    out.push('\n');
  }
  out
}

fn truncate_tokens(text: &str, max_tokens: usize, include_marker: bool) -> String {
  if max_tokens == 0 {
    return String::new();
  }

  let estimated = estimate_tokens(text);
  if estimated <= max_tokens {
    return text.to_string();
  }

  // Fast approx: 1 token ~= 4 chars.
  let max_chars = max_tokens.saturating_mul(4);
  if max_chars == 0 {
    return String::new();
  }

  let chars: Vec<char> = text.chars().collect();
  if chars.len() <= max_chars {
    return text.to_string();
  }

  let head_chars = max_chars / 2;
  let tail_chars = max_chars.saturating_sub(head_chars);

  let head = chars[..head_chars].iter().collect::<String>();
  let tail = chars[chars.len() - tail_chars..].iter().collect::<String>();

  let omitted_chars = chars.len().saturating_sub(head_chars + tail_chars);
  let omitted_tokens = estimated.saturating_sub(max_tokens);

  if include_marker {
    format!("{head}\n... (~{omitted_tokens} tokens / {omitted_chars} chars omitted) ...\n{tail}")
  } else {
    format!("{head}{tail}")
  }
}

fn estimate_tokens(text: &str) -> usize {
  // Keep deterministic and dependency-free for phase-0.
  // Use ceil(chars / 4), minimum 1 for non-empty strings.
  let chars = text.chars().count();
  if chars == 0 {
    return 0;
  }
  let est = chars.div_ceil(4);
  est.max(1)
}

pub fn maybe_truncated<'a>(text: &'a str, policy: TruncationPolicy) -> Cow<'a, str> {
  let truncated = truncate_text(text, policy);
  if truncated == text {
    Cow::Borrowed(text)
  } else {
    Cow::Owned(truncated)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn lines_policy_keeps_head_and_tail() {
    let input = "a\nb\nc\nd\ne\nf\n";
    let out = formatted_truncate_text(input, TruncationPolicy::Lines(4));
    assert!(out.contains("a\nb"));
    assert!(out.contains("e\nf"));
    assert!(out.contains("lines omitted"));
  }

  #[test]
  fn token_policy_truncates() {
    let input = "x".repeat(2000);
    let out = formatted_truncate_text(&input, TruncationPolicy::Tokens(100));
    assert!(out.contains("tokens"));
    assert!(out.len() < input.len());
  }
}
