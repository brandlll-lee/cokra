use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::agent::team_runtime::runtime_for_thread;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct SpawnAgentHandler;

#[derive(Debug, Deserialize)]
struct SpawnAgentArgs {
  #[serde(alias = "initial_task")]
  task: Option<String>,
  #[serde(alias = "input", alias = "prompt")]
  message: Option<String>,
  #[serde(alias = "name")]
  nickname: Option<String>,
  role: Option<String>,
  #[serde(alias = "type")]
  agent_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct SpawnAgentResult {
  thread_id: String,
  agent_id: String,
  nickname: Option<String>,
  role: String,
  status: String,
}

fn parse_leading_identity_token(text: &str) -> Option<String> {
  let trimmed = text.trim_start();
  let first = trimmed.chars().next()?;
  let (body, quoted) = match first {
    '“' => (&trimmed[first.len_utf8()..], Some('”')),
    '"' => (&trimmed[first.len_utf8()..], Some('"')),
    '\'' => (&trimmed[first.len_utf8()..], Some('\'')),
    _ => (trimmed, None),
  };

  let candidate = if let Some(close) = quoted {
    let end = body.find(close)?;
    body[..end].trim().to_string()
  } else {
    body
      .chars()
      .take_while(|ch| {
        !matches!(
          ch,
          '，' | ',' | '。' | '.' | '：' | ':' | '；' | ';' | ' ' | '\n' | '\r' | '）' | ')'
        )
      })
      .collect::<String>()
      .trim()
      .to_string()
  };

  if candidate.is_empty() {
    return None;
  }

  Some(candidate)
}

fn extract_identity_after_marker(text: &str, markers: &[&str]) -> Option<String> {
  for marker in markers {
    let Some(start) = text.find(marker) else {
      continue;
    };
    let rest = &text[start + marker.len()..];
    if let Some(candidate) = parse_leading_identity_token(rest) {
      return Some(candidate);
    }
  }
  None
}

fn infer_nickname_from_message(message: &str) -> Option<String> {
  // Tradeoff: this stays heuristic because model outputs are not schema-stable,
  // but we now only trust identity-shaped phrases instead of the first quoted
  // string. This avoids mistaking the discussion topic for the teammate name.
  extract_identity_after_marker(
    message,
    &[
      "你现在是",
      "你是",
      "请你扮演",
      "成员“",
      "成员\"",
      "You are now",
      "You are",
      "Act as",
    ],
  )
}

fn resolve_message(
  args: SpawnAgentArgs,
) -> Result<(String, Option<String>, String), FunctionCallError> {
  let task = args.task.map(|value| value.trim().to_string());
  let message = args.message.map(|value| value.trim().to_string());

  let message = match (task, message) {
    (Some(task), Some(message)) if !task.is_empty() && !message.is_empty() => {
      return Err(FunctionCallError::RespondToModel(
        "spawn_agent accepts either `task` or `message`, not both".to_string(),
      ));
    }
    (Some(task), _) if !task.is_empty() => task,
    (_, Some(message)) if !message.is_empty() => message,
    _ => {
      return Err(FunctionCallError::RespondToModel(
        "spawn_agent requires a non-empty `task` or `message`".to_string(),
      ));
    }
  };

  let role = args
    .agent_type
    .or(args.role)
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| "default".to_string());

  let nickname = args
    .nickname
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty())
    .or_else(|| infer_nickname_from_message(&message));

  Ok((message, nickname, role))
}

#[async_trait]
impl ToolHandler for SpawnAgentHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: SpawnAgentArgs = invocation.parse_arguments()?;
    let runtime = invocation
      .runtime
      .ok_or_else(|| FunctionCallError::Fatal("spawn_agent missing runtime context".to_string()))?;
    let team_runtime = runtime_for_thread(&runtime.thread_id).ok_or_else(|| {
      FunctionCallError::Execution("spawn_agent runtime is not configured".to_string())
    })?;
    let (message, nickname, role) = resolve_message(args)?;
    let thread_id = team_runtime
      .spawn_agent(&runtime.thread_id, message, nickname.clone(), role.clone())
      .await
      .map_err(|err| FunctionCallError::Execution(err.to_string()))?;

    let mut out = ToolOutput::success(
      serde_json::to_string(&SpawnAgentResult {
        thread_id: thread_id.to_string(),
        agent_id: thread_id.to_string(),
        nickname,
        role,
        status: "running".to_string(),
      })
      .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize spawn result: {err}"))
      })?,
    );
    Ok(out.with_id(invocation.id))
  }
}

#[cfg(test)]
mod tests {
  use super::SpawnAgentArgs;
  use super::infer_nickname_from_message;
  use super::resolve_message;

  #[test]
  fn infers_nickname_from_chinese_quotes() {
    let nickname = infer_nickname_from_message(
      "你现在是“艾许”，一名资深系统架构师。请深入分析 core/ 和 protocol/。",
    );
    assert_eq!(nickname.as_deref(), Some("艾许"));
  }

  #[test]
  fn infers_nickname_from_ascii_quotes() {
    let nickname =
      infer_nickname_from_message("You are \"Six Sparrow\", focused on tests and tools.");
    assert_eq!(nickname.as_deref(), Some("Six Sparrow"));
  }

  #[test]
  fn infers_nickname_from_prefix_without_quotes() {
    let nickname = infer_nickname_from_message("你现在是六雀，负责梳理测试与工具。");
    assert_eq!(nickname.as_deref(), Some("六雀"));
  }

  #[test]
  fn resolve_message_prefers_explicit_nickname() {
    let (message, nickname, role) = resolve_message(SpawnAgentArgs {
      task: Some("你现在是“艾许”，分析核心架构。".to_string()),
      message: None,
      nickname: Some("手动名称".to_string()),
      role: None,
      agent_type: Some("explorer".to_string()),
    })
    .expect("spawn args should parse");

    assert_eq!(message, "你现在是“艾许”，分析核心架构。");
    assert_eq!(nickname.as_deref(), Some("手动名称"));
    assert_eq!(role, "explorer");
  }

  #[test]
  fn resolve_message_falls_back_to_inferred_nickname() {
    let (_, nickname, _) = resolve_message(SpawnAgentArgs {
      task: Some("你现在是“六雀”，负责梳理测试与工具。".to_string()),
      message: None,
      nickname: None,
      role: None,
      agent_type: None,
    })
    .expect("spawn args should parse");

    assert_eq!(nickname.as_deref(), Some("六雀"));
  }

  #[test]
  fn infers_nickname_from_identity_phrase_not_topic_quotes() {
    let nickname = infer_nickname_from_message(
      "请加入团队讨论：“特斯拉 Model 3 vs 小米 SU7 选购探讨”。你现在是“洋洋”，请从技术栈角度分析。",
    );
    assert_eq!(nickname.as_deref(), Some("洋洋"));
  }
}
