use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fmt::Write as _;

use serde_json::Value;

use crate::model::ChatRequest;
use crate::model::Message;
use crate::model::ModelClient;
use crate::model::ModelError;
use crate::model::ToolCall;
use crate::model::Usage;

const SUMMARY_MARKER: &str = "[cokra-summary-v1]";
const SUMMARIZATION_SYSTEM_PROMPT: &str = "You produce structured context checkpoint summaries for an agentic coding session. Do not continue the conversation. Preserve exact file paths, function names, requirements, and unresolved issues.";
const SUMMARIZATION_PROMPT: &str = r#"The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What the user is trying to accomplish]

## Constraints
- [Constraints, requirements, or preferences]
- [(none) if there are no explicit constraints]

## Progress
### Done
- [Completed work]

### In Progress
- [Current work]

### Blocked
- [Current blockers]

## Next
1. [Ordered next step]

## Context
- [Critical context needed to continue]
- [Include file operations as "Files read: ..." and "Files modified: ..."]

Keep the summary concise, factual, and continuation-ready."#;
const UPDATE_SUMMARIZATION_PROMPT: &str = r#"The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the structured summary while preserving important prior context.

Rules:
- Preserve still-relevant information from the previous summary.
- Move completed work from In Progress to Done when appropriate.
- Update Next so it reflects the latest state.
- Preserve exact file paths, function names, requirements, and unresolved issues.

Use this EXACT format:

## Goal
[What the user is trying to accomplish]

## Constraints
- [Constraints, requirements, or preferences]
- [(none) if there are no explicit constraints]

## Progress
### Done
- [Completed work]

### In Progress
- [Current work]

### Blocked
- [Current blockers]

## Next
1. [Ordered next step]

## Context
- [Critical context needed to continue]
- [Include file operations as "Files read: ..." and "Files modified: ..."]

Keep the summary concise, factual, and continuation-ready."#;

#[derive(Debug, Clone)]
pub(crate) struct CompactionSettings {
  pub enabled: bool,
  pub reserve_tokens: usize,
  pub keep_recent_tokens: usize,
  pub max_summary_tokens: usize,
  pub prompt_override: Option<String>,
}

impl Default for CompactionSettings {
  fn default() -> Self {
    Self {
      enabled: true,
      reserve_tokens: 16_000,
      keep_recent_tokens: 32_000,
      max_summary_tokens: 2_048,
      prompt_override: None,
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CompactionFileOps {
  pub read_files: Vec<String>,
  pub modified_files: Vec<String>,
  pub deleted_files: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CompactionPlan {
  pub first_kept_index: usize,
  pub messages_to_summarize: Vec<Message>,
  pub kept_messages: Vec<Message>,
  pub previous_summary: Option<String>,
  pub file_ops: CompactionFileOps,
  pub tokens_before_est: usize,
  pub tokens_after_est: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct CompactionRunResult {
  pub summary: String,
  pub usage: Usage,
}

#[derive(Debug, Clone)]
pub(crate) struct AppliedCompactionResult {
  pub summary: String,
  pub usage: Usage,
  pub compacted_history: Vec<Message>,
  pub tokens_before_est: usize,
  pub tokens_after_est: usize,
}

pub(crate) fn estimate_message_tokens(msg: &Message) -> usize {
  let text_len = msg.text().map_or(0usize, |s| s.chars().count());
  if text_len == 0 {
    1
  } else {
    text_len.div_ceil(4)
  }
}

pub(crate) fn estimate_messages_tokens(messages: &[Message]) -> usize {
  messages.iter().map(estimate_message_tokens).sum()
}

pub(crate) fn is_summary_message(message: &Message) -> bool {
  match message {
    Message::User(text) => text.starts_with(SUMMARY_MARKER),
    _ => false,
  }
}

pub(crate) fn extract_summary_text(message: &Message) -> Option<String> {
  match message {
    Message::User(text) if text.starts_with(SUMMARY_MARKER) => Some(
      text[SUMMARY_MARKER.len()..]
        .trim_start_matches('\n')
        .trim()
        .to_string(),
    ),
    _ => None,
  }
}

pub(crate) fn make_summary_message(summary: impl Into<String>) -> Message {
  Message::User(format!("{SUMMARY_MARKER}\n{}", summary.into().trim()))
}

pub(crate) fn first_non_system_index(history: &[Message]) -> Option<usize> {
  history
    .iter()
    .position(|message| !matches!(message, Message::System(_)))
}

pub(crate) fn find_safe_tail_start_index(
  history: &[Message],
  boundary_start: usize,
  keep_recent_tokens: usize,
  allow_boundary_start: bool,
) -> Option<usize> {
  if boundary_start > history.len() {
    return None;
  }
  if boundary_start == history.len() {
    return Some(history.len());
  }

  let valid_boundaries = compute_valid_kept_boundaries(history, boundary_start);
  let min_boundary = if allow_boundary_start {
    boundary_start
  } else {
    boundary_start.saturating_add(1)
  };
  if min_boundary > history.len() {
    return None;
  }

  let mut suffix_tokens = vec![0usize; history.len() + 1];
  for index in (boundary_start..history.len()).rev() {
    suffix_tokens[index] = suffix_tokens[index + 1] + estimate_message_tokens(&history[index]);
  }

  if allow_boundary_start && suffix_tokens[boundary_start] <= keep_recent_tokens {
    return Some(boundary_start);
  }
  if !allow_boundary_start && suffix_tokens[boundary_start] <= keep_recent_tokens {
    return None;
  }

  let mut best_boundary = None;
  let mut best_distance = usize::MAX;
  let mut best_tail_tokens = 0usize;

  for boundary in min_boundary..=history.len() {
    if !valid_boundaries[boundary] {
      continue;
    }
    let tail_tokens = suffix_tokens[boundary];
    let distance = tail_tokens.abs_diff(keep_recent_tokens);
    if distance < best_distance || (distance == best_distance && tail_tokens > best_tail_tokens) {
      best_distance = distance;
      best_tail_tokens = tail_tokens;
      best_boundary = Some(boundary);
    }
  }

  best_boundary
}

pub(crate) fn prepare_compaction(
  history: &[Message],
  settings: &CompactionSettings,
) -> Option<CompactionPlan> {
  if !settings.enabled {
    return None;
  }

  let first_non_system = first_non_system_index(history)?;
  let previous_summary_index = history
    .iter()
    .enumerate()
    .skip(first_non_system)
    .rev()
    .find_map(|(index, message)| is_summary_message(message).then_some(index));
  let previous_summary = previous_summary_index
    .and_then(|index| extract_summary_text(&history[index]))
    .filter(|summary| !summary.is_empty());
  let boundary_start = previous_summary_index.map_or(first_non_system, |index| index + 1);
  if boundary_start >= history.len() {
    return None;
  }

  let first_kept_index =
    find_safe_tail_start_index(history, boundary_start, settings.keep_recent_tokens, false)?;
  if first_kept_index <= boundary_start || first_kept_index > history.len() {
    return None;
  }

  let messages_to_summarize = history[boundary_start..first_kept_index]
    .iter()
    .filter(|message| !matches!(message, Message::System(_)) && !is_summary_message(message))
    .cloned()
    .collect::<Vec<_>>();
  if messages_to_summarize.is_empty() {
    return None;
  }

  let kept_messages = history[first_kept_index..]
    .iter()
    .filter(|message| !matches!(message, Message::System(_)) && !is_summary_message(message))
    .cloned()
    .collect::<Vec<_>>();
  let file_ops = extract_file_ops(&messages_to_summarize);
  let system_tokens = history
    .iter()
    .filter(|message| matches!(message, Message::System(_)))
    .map(estimate_message_tokens)
    .sum::<usize>();

  Some(CompactionPlan {
    first_kept_index,
    messages_to_summarize,
    kept_messages: kept_messages.clone(),
    previous_summary,
    file_ops,
    tokens_before_est: estimate_messages_tokens(history),
    tokens_after_est: system_tokens
      .saturating_add(settings.max_summary_tokens)
      .saturating_add(estimate_messages_tokens(&kept_messages)),
  })
}

pub(crate) async fn run_compaction(
  model_client: &ModelClient,
  model: &str,
  plan: &CompactionPlan,
  settings: &CompactionSettings,
) -> Result<CompactionRunResult, ModelError> {
  let prompt = build_compaction_prompt(plan, settings);
  let response = model_client
    .chat(ChatRequest {
      model: model.to_string(),
      messages: vec![
        Message::System(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
        Message::User(prompt),
      ],
      temperature: Some(0.0),
      max_tokens: Some(settings.max_summary_tokens.min(u32::MAX as usize) as u32),
      tools: None,
      tool_choice: None,
      stream: false,
      ..Default::default()
    })
    .await?;
  let usage = response.usage.clone();

  let summary = response
    .choices
    .into_iter()
    .find_map(|choice| choice.message.content)
    .map(|text| text.trim().to_string())
    .filter(|text| !text.is_empty())
    .ok_or_else(|| {
      ModelError::InvalidResponse("compaction summary response did not contain text".to_string())
    })?;

  Ok(CompactionRunResult { summary, usage })
}

pub(crate) fn apply_compaction(
  history: &[Message],
  summary: &str,
  plan: &CompactionPlan,
) -> Vec<Message> {
  let mut compacted = history
    .iter()
    .filter(|message| matches!(message, Message::System(_)))
    .cloned()
    .collect::<Vec<_>>();
  compacted.push(make_summary_message(summary));
  compacted.extend(plan.kept_messages.clone());
  compacted
}

pub(crate) async fn compact_history_with_summary(
  model_client: &ModelClient,
  model: &str,
  history: &[Message],
  settings: &CompactionSettings,
) -> Result<Option<AppliedCompactionResult>, ModelError> {
  let Some(plan) = prepare_compaction(history, settings) else {
    return Ok(None);
  };

  let compaction = run_compaction(model_client, model, &plan, settings).await?;
  let compacted_history = apply_compaction(history, &compaction.summary, &plan);

  Ok(Some(AppliedCompactionResult {
    summary: compaction.summary,
    usage: compaction.usage,
    compacted_history,
    tokens_before_est: plan.tokens_before_est,
    tokens_after_est: plan.tokens_after_est,
  }))
}

fn compute_valid_kept_boundaries(history: &[Message], boundary_start: usize) -> Vec<bool> {
  let mut valid = vec![false; history.len() + 1];
  valid[history.len()] = true;

  let mut unresolved_tool_results = HashSet::new();
  for index in (boundary_start..history.len()).rev() {
    match &history[index] {
      Message::Tool { tool_call_id, .. } => {
        unresolved_tool_results.insert(tool_call_id.clone());
        valid[index] = false;
      }
      Message::Assistant { tool_calls, .. } => {
        if let Some(tool_calls) = tool_calls {
          for call in tool_calls {
            unresolved_tool_results.remove(&call.id);
          }
        }
        valid[index] = unresolved_tool_results.is_empty();
      }
      _ => {
        valid[index] = unresolved_tool_results.is_empty();
      }
    }
  }

  valid
}

fn build_compaction_prompt(plan: &CompactionPlan, settings: &CompactionSettings) -> String {
  let mut prompt = String::new();
  let _ = write!(
    prompt,
    "<conversation>\n{}\n</conversation>\n",
    serialize_messages(&plan.messages_to_summarize)
  );

  if let Some(previous_summary) = &plan.previous_summary {
    let _ = write!(
      prompt,
      "\n<previous-summary>\n{}\n</previous-summary>\n",
      previous_summary
    );
  }

  let _ = write!(
    prompt,
    "\n<file-operations>\n{}\n</file-operations>\n",
    render_file_ops(&plan.file_ops)
  );

  let base_prompt = if plan.previous_summary.is_some() {
    UPDATE_SUMMARIZATION_PROMPT
  } else {
    SUMMARIZATION_PROMPT
  };
  prompt.push('\n');
  prompt.push_str(base_prompt);

  if let Some(override_prompt) = settings
    .prompt_override
    .as_deref()
    .map(str::trim)
    .filter(|prompt| !prompt.is_empty())
  {
    let _ = write!(
      prompt,
      "\n\nAdditional focus for this summary:\n{}",
      override_prompt
    );
  }

  prompt
}

fn serialize_messages(messages: &[Message]) -> String {
  let mut out = String::new();
  for (index, message) in messages.iter().enumerate() {
    if index > 0 {
      out.push_str("\n\n");
    }
    let _ = write!(out, "[{}] {}", index + 1, message_role(message));
    match message {
      Message::System(text) | Message::User(text) => {
        let _ = write!(out, "\n{}", text.trim());
      }
      Message::Assistant {
        content,
        tool_calls,
      } => {
        if let Some(content) = content
          .as_deref()
          .map(str::trim)
          .filter(|text| !text.is_empty())
        {
          let _ = write!(out, "\n{}", content);
        }
        if let Some(tool_calls) = tool_calls {
          for call in tool_calls {
            let _ = write!(
              out,
              "\n<tool-call id=\"{}\" name=\"{}\">{}</tool-call>",
              call.id, call.function.name, call.function.arguments
            );
          }
        }
      }
      Message::Tool {
        tool_call_id,
        content,
      } => {
        let _ = write!(
          out,
          "\n<tool-result id=\"{}\">{}</tool-result>",
          tool_call_id,
          content.trim()
        );
      }
    }
  }
  out
}

fn message_role(message: &Message) -> &'static str {
  match message {
    Message::System(_) => "system",
    Message::User(_) => "user",
    Message::Assistant { .. } => "assistant",
    Message::Tool { .. } => "tool",
  }
}

fn render_file_ops(file_ops: &CompactionFileOps) -> String {
  let mut rendered = String::new();
  let read = if file_ops.read_files.is_empty() {
    "(none)".to_string()
  } else {
    file_ops.read_files.join(", ")
  };
  let modified = if file_ops.modified_files.is_empty() {
    "(none)".to_string()
  } else {
    file_ops.modified_files.join(", ")
  };
  let deleted = if file_ops.deleted_files.is_empty() {
    "(none)".to_string()
  } else {
    file_ops.deleted_files.join(", ")
  };

  let _ = write!(
    rendered,
    "Files read: {}\nFiles modified: {}\nFiles deleted: {}",
    read, modified, deleted
  );
  rendered
}

fn extract_file_ops(messages: &[Message]) -> CompactionFileOps {
  let mut read_files = BTreeSet::new();
  let mut modified_files = BTreeSet::new();
  let mut deleted_files = BTreeSet::new();

  for message in messages {
    let Message::Assistant {
      tool_calls: Some(tool_calls),
      ..
    } = message
    else {
      continue;
    };

    for call in tool_calls {
      extract_file_ops_from_call(
        call,
        &mut read_files,
        &mut modified_files,
        &mut deleted_files,
      );
    }
  }

  CompactionFileOps {
    read_files: read_files.into_iter().collect(),
    modified_files: modified_files.into_iter().collect(),
    deleted_files: deleted_files.into_iter().collect(),
  }
}

fn extract_file_ops_from_call(
  call: &ToolCall,
  read_files: &mut BTreeSet<String>,
  modified_files: &mut BTreeSet<String>,
  deleted_files: &mut BTreeSet<String>,
) {
  let Ok(arguments) = serde_json::from_str::<Value>(&call.function.arguments) else {
    return;
  };

  match call.function.name.as_str() {
    "read_file" => {
      if let Some(path) = string_field(&arguments, "file_path") {
        read_files.insert(path.to_string());
      }
    }
    "read_many_files" => {
      for path in string_array_field(&arguments, "paths") {
        read_files.insert(path);
      }
    }
    "view_image" => {
      if let Some(path) = string_field(&arguments, "path") {
        read_files.insert(path.to_string());
      }
    }
    "write_file" | "edit_file" => {
      if let Some(path) = string_field(&arguments, "file_path") {
        modified_files.insert(path.to_string());
      }
    }
    "apply_patch" => {
      if let Some(patch) = string_field(&arguments, "patch")
        && let Ok(parsed_patch) = cokra_apply_patch::parse_patch(patch)
      {
        for hunk in parsed_patch.hunks {
          match hunk {
            cokra_apply_patch::Hunk::AddFile { path, .. } => {
              modified_files.insert(path.display().to_string());
            }
            cokra_apply_patch::Hunk::DeleteFile { path } => {
              deleted_files.insert(path.display().to_string());
            }
            cokra_apply_patch::Hunk::UpdateFile {
              path, move_path, ..
            } => {
              modified_files.insert(path.display().to_string());
              if let Some(move_path) = move_path {
                modified_files.insert(move_path.display().to_string());
              }
            }
          }
        }
      }
    }
    _ => {}
  }
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
  value.get(key)?.as_str()
}

fn string_array_field(value: &Value, key: &str) -> Vec<String> {
  value
    .get(key)
    .and_then(Value::as_array)
    .into_iter()
    .flat_map(|items| items.iter())
    .filter_map(Value::as_str)
    .map(str::to_string)
    .collect()
}

#[cfg(test)]
mod tests {
  use super::CompactionSettings;
  use super::Message;
  use super::apply_compaction;
  use super::extract_summary_text;
  use super::find_safe_tail_start_index;
  use super::make_summary_message;
  use super::prepare_compaction;
  use crate::model::ToolCall;
  use crate::model::ToolCallFunction;

  fn tool_call(id: &str, name: &str, arguments: &str) -> ToolCall {
    ToolCall {
      id: id.to_string(),
      call_type: "function".to_string(),
      function: ToolCallFunction {
        name: name.to_string(),
        arguments: arguments.to_string(),
      },
      provider_meta: None,
    }
  }

  #[test]
  fn safe_tail_start_never_begins_with_tool_result() {
    let history = vec![
      Message::User("user".to_string()),
      Message::Assistant {
        content: Some("calling tool".to_string()),
        tool_calls: Some(vec![tool_call(
          "call-1",
          "read_file",
          r#"{"file_path":"a.rs"}"#,
        )]),
      },
      Message::Tool {
        tool_call_id: "call-1".to_string(),
        content: "file content".to_string(),
      },
      Message::Assistant {
        content: Some("done".to_string()),
        tool_calls: None,
      },
    ];

    let first_kept = find_safe_tail_start_index(&history, 0, 2, false).unwrap();
    assert_eq!(first_kept, 3);
  }

  #[test]
  fn prepare_compaction_preserves_previous_summary_incrementally() {
    let history = vec![
      Message::System("sys".to_string()),
      make_summary_message("## Goal\nold"),
      Message::User("new task".to_string()),
      Message::Assistant {
        content: Some("working".to_string()),
        tool_calls: None,
      },
      Message::User("latest".to_string()),
    ];

    let plan = prepare_compaction(
      &history,
      &CompactionSettings {
        keep_recent_tokens: 1,
        ..CompactionSettings::default()
      },
    )
    .expect("plan should exist");

    assert_eq!(plan.first_kept_index, 4);
    assert_eq!(plan.previous_summary.as_deref(), Some("## Goal\nold"));
  }

  #[test]
  fn apply_compaction_replaces_prior_summary_with_new_one() {
    let history = vec![
      Message::System("sys".to_string()),
      make_summary_message("old"),
      Message::User("keep".to_string()),
    ];
    let plan = prepare_compaction(
      &history,
      &CompactionSettings {
        keep_recent_tokens: 0,
        ..CompactionSettings::default()
      },
    )
    .expect("plan should exist");

    let compacted = apply_compaction(&history, "new summary", &plan);
    assert_eq!(compacted.len(), 2);
    assert!(matches!(compacted[0], Message::System(_)));
    assert_eq!(
      extract_summary_text(&compacted[1]).as_deref(),
      Some("new summary")
    );
  }
}
