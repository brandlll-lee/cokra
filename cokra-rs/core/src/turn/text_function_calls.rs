use cokra_protocol::FunctionCall;
use cokra_protocol::FunctionCallEvent;

const START_TAG: &str = "<function_calls>";
const END_TAG: &str = "</function_calls>";

#[derive(Debug, Default)]
pub(crate) struct ParsedTextFunctionCalls {
  pub(crate) visible_text: String,
  pub(crate) calls: Vec<FunctionCallEvent>,
}

/// Best-effort parser for "XML-ish" Codex-style function call blocks emitted as plain text:
///
/// ```text
/// <function_calls>
///   <invoke name="spawn_agent">
///     <parameter name="name">codex_expert</parameter>
///     <parameter name="initial_task">...</parameter>
///   </invoke>
/// </function_calls>
/// ```
///
/// This exists to support providers/models that ignore OpenAI-compatible structured
/// tool calling but still follow the textual convention.
pub(crate) fn parse_text_function_calls(input: &str) -> ParsedTextFunctionCalls {
  let mut out = ParsedTextFunctionCalls::default();
  let mut cursor = 0usize;
  let mut call_seq = 0usize;

  while let Some(rel_start) = input[cursor..].find(START_TAG) {
    let start = cursor + rel_start;
    out.visible_text.push_str(&input[cursor..start]);

    let after_start = start + START_TAG.len();
    let Some(rel_end) = input[after_start..].find(END_TAG) else {
      // Unterminated block: treat the remainder as visible text.
      out.visible_text.push_str(&input[start..]);
      return out;
    };
    let end = after_start + rel_end;

    let block = &input[after_start..end];
    parse_invoke_block(block, &mut call_seq, &mut out.calls);

    cursor = end + END_TAG.len();
  }

  out.visible_text.push_str(&input[cursor..]);
  out
}

#[derive(Debug, Default)]
pub(crate) struct FunctionCallsTextFilter {
  in_block: bool,
  buffer: String,
}

impl FunctionCallsTextFilter {
  pub(crate) fn new() -> Self {
    Self::default()
  }

  pub(crate) fn filter_visible(&mut self, delta: &str) -> String {
    if delta.is_empty() {
      return String::new();
    }

    self.buffer.push_str(delta);
    let mut emitted = String::new();

    loop {
      if !self.in_block {
        if let Some(pos) = self.buffer.find(START_TAG) {
          emitted.push_str(&self.buffer[..pos]);
          self.buffer.drain(..pos + START_TAG.len());
          self.in_block = true;
          continue;
        }

        // No start tag found. Avoid emitting a partial `<function_calls>` prefix
        // that may be completed by the next chunk.
        let keep = suffix_len_matching_prefix(&self.buffer, START_TAG);
        let emit_end = self.buffer.len().saturating_sub(keep);
        emitted.push_str(&self.buffer[..emit_end]);
        self.buffer.drain(..emit_end);
        break;
      }

      // Suppressing: drop everything until the closing tag is observed.
      if let Some(pos) = self.buffer.find(END_TAG) {
        self.buffer.drain(..pos + END_TAG.len());
        self.in_block = false;
        continue;
      }

      // Cap the suppressed buffer so it doesn't grow unbounded while we're inside a block.
      // Tradeoff: we keep only enough suffix to detect the end tag across chunk boundaries.
      let keep = suffix_len_matching_prefix(&self.buffer, END_TAG);
      if keep < self.buffer.len() {
        let drain_len = self.buffer.len() - keep;
        self.buffer.drain(..drain_len);
      }
      break;
    }

    emitted
  }
}

fn suffix_len_matching_prefix(haystack: &str, needle: &str) -> usize {
  let max = needle.len().saturating_sub(1);
  let max = max.min(haystack.len());
  for len in (1..=max).rev() {
    if haystack.ends_with(&needle[..len]) {
      return len;
    }
  }
  0
}

fn sanitize_tool_name(raw: &str) -> String {
  let trimmed = raw.trim();
  let mut s = trimmed;
  if let Some(idx) = s.find('<') {
    s = &s[..idx];
  }
  if let Some(idx) = s.find(|ch: char| ch.is_whitespace()) {
    s = &s[..idx];
  }
  s.chars()
    .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    .collect::<String>()
}

fn parse_attr_value(tag: &str, key: &str) -> Option<String> {
  let needle = format!("{key}=\"");
  if let Some(start) = tag.find(&needle) {
    let rest = &tag[start + needle.len()..];
    if let Some(end) = rest.find('"') {
      return Some(rest[..end].to_string());
    }
    // Malformed: missing closing quote; fall back to tag boundary heuristics.
    let end = rest
      .find(|ch: char| ch == '>' || ch.is_whitespace())
      .unwrap_or(rest.len());
    return Some(rest[..end].to_string());
  }
  let needle = format!("{key}='");
  if let Some(start) = tag.find(&needle) {
    let rest = &tag[start + needle.len()..];
    if let Some(end) = rest.find('\'') {
      return Some(rest[..end].to_string());
    }
    let end = rest
      .find(|ch: char| ch == '>' || ch.is_whitespace())
      .unwrap_or(rest.len());
    return Some(rest[..end].to_string());
  }
  None
}

fn parse_invoke_block(block: &str, call_seq: &mut usize, out: &mut Vec<FunctionCallEvent>) {
  let mut cursor = 0usize;
  while let Some(rel) = block[cursor..].find("<invoke") {
    let invoke_start = cursor + rel;
    let Some(tag_end_rel) = block[invoke_start..].find('>') else {
      return;
    };
    let tag_end = invoke_start + tag_end_rel;
    let invoke_tag = &block[invoke_start..=tag_end];
    let name_raw = parse_attr_value(invoke_tag, "name").unwrap_or_default();
    let tool_name = sanitize_tool_name(&name_raw);
    if tool_name.is_empty() {
      cursor = tag_end + 1;
      continue;
    }

    let body_start = tag_end + 1;
    let Some(body_end_rel) = block[body_start..].find("</invoke>") else {
      return;
    };
    let body_end = body_start + body_end_rel;
    let body = &block[body_start..body_end];

    let args = parse_parameters(body);
    let mapped = map_args_for_tool(&tool_name, args);

    *call_seq += 1;
    let call_id = format!("text_call_{}", *call_seq);
    let arguments = serde_json::to_string(&mapped).unwrap_or_else(|_| "{}".to_string());

    out.push(FunctionCallEvent {
      id: call_id,
      call_type: "function".to_string(),
      function: FunctionCall {
        name: tool_name,
        arguments,
      },
      thought_signature: None,
    });

    cursor = body_end + "</invoke>".len();
  }
}

fn parse_parameters(body: &str) -> std::collections::BTreeMap<String, serde_json::Value> {
  let mut args = std::collections::BTreeMap::new();
  let mut cursor = 0usize;

  while let Some(rel) = body[cursor..].find("<parameter") {
    let start = cursor + rel;
    let Some(tag_end_rel) = body[start..].find('>') else {
      break;
    };
    let tag_end = start + tag_end_rel;
    let tag = &body[start..=tag_end];
    let name = parse_attr_value(tag, "name").unwrap_or_default();
    if name.is_empty() {
      cursor = tag_end + 1;
      continue;
    }

    let value_start = tag_end + 1;
    let Some(close_rel) = body[value_start..].find("</parameter>") else {
      break;
    };
    let value_end = value_start + close_rel;
    let value = body[value_start..value_end].trim().to_string();
    args.insert(name, serde_json::Value::String(value));
    cursor = value_end + "</parameter>".len();
  }

  args
}

fn map_args_for_tool(
  tool_name: &str,
  mut raw: std::collections::BTreeMap<String, serde_json::Value>,
) -> serde_json::Value {
  // Map Codex-style aliases to Cokra tool schemas (additionalProperties=false).
  match tool_name {
    "spawn_agent" => {
      if raw.get("task").is_none() {
        if let Some(value) = raw.remove("initial_task") {
          raw.insert("task".to_string(), value);
        }
      }
      if raw.get("nickname").is_none() {
        if let Some(value) = raw.remove("name") {
          raw.insert("nickname".to_string(), value);
        }
      }
    }
    "send_input" => {
      if raw.get("agent_id").is_none() {
        if let Some(value) = raw.remove("agent") {
          raw.insert("agent_id".to_string(), value);
        }
      }
      if raw.get("message").is_none() {
        if let Some(value) = raw.remove("input") {
          raw.insert("message".to_string(), value);
        }
      }
    }
    "close_agent" => {
      if raw.get("agent_id").is_none() {
        if let Some(value) = raw.remove("agent") {
          raw.insert("agent_id".to_string(), value);
        }
      }
    }
    "wait" => {
      if raw.get("agent_ids").is_none() {
        if let Some(value) = raw.remove("agents") {
          raw.insert("agent_ids".to_string(), value);
        }
      }
    }
    _ => {}
  }

  serde_json::Value::Object(raw.into_iter().collect())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_spawn_agent_and_strips_block() {
    let input = r#"preamble
<function_calls>
  <invoke name="spawn_agent">
    <parameter name="name">codex_expert</parameter>
    <parameter name="initial_task">do work</parameter>
  </invoke>
</function_calls>
"#;

    let parsed = parse_text_function_calls(input);
    assert_eq!(parsed.visible_text.trim(), "preamble");
    assert_eq!(parsed.calls.len(), 1);
    assert_eq!(parsed.calls[0].function.name, "spawn_agent");
    assert!(
      parsed.calls[0]
        .function
        .arguments
        .contains("\"nickname\":\"codex_expert\"")
    );
    assert!(
      parsed.calls[0]
        .function
        .arguments
        .contains("\"task\":\"do work\"")
    );
  }

  #[test]
  fn sanitizes_malformed_invoke_tag() {
    let input = r#"<function_calls>
<invoke name="spawn_agent</parameter>>
<parameter name="name">x</parameter>
<parameter name="initial_task">y</parameter>
</invoke>
</function_calls>"#;

    let parsed = parse_text_function_calls(input);
    assert_eq!(parsed.calls.len(), 1);
    assert_eq!(parsed.calls[0].function.name, "spawn_agent");
  }

  #[test]
  fn streaming_filter_hides_function_call_blocks() {
    let mut filter = FunctionCallsTextFilter::new();
    let a = filter.filter_visible("hello\n<function_");
    let b =
      filter.filter_visible("calls>\n<invoke name=\"wait\"></invoke>\n</function_calls>\nbye");
    assert_eq!(a, "hello\n");
    assert_eq!(b.trim(), "bye");
  }
}
