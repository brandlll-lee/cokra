//! Hooks 系统核心类型定义。
//!
//! 1:1 复刻 codex `hooks/src/types.rs`，精简为 cokra 所需事件集合。

use std::path::PathBuf;
use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use futures::future::BoxFuture;
use serde::Deserialize;
use serde::Serialize;

// ── Hook 函数签名 ─────────────────────────────────────────────────────────────

/// Hook 异步函数类型。
pub type HookFn = Arc<dyn for<'a> Fn(&'a HookPayload) -> BoxFuture<'a, HookResult> + Send + Sync>;

// ── Hook 执行结果 ─────────────────────────────────────────────────────────────

/// Hook 执行结果，1:1 codex `HookResult`。
#[derive(Debug)]
pub enum HookResult {
  /// 成功执行，继续后续 hooks 和工具调用。
  Success,
  /// 执行失败，但继续后续 hooks（不中断工具调用）。
  FailedContinue(Box<dyn std::error::Error + Send + Sync + 'static>),
  /// 执行失败，停止所有后续 hooks，中断工具调用。
  FailedAbort(Box<dyn std::error::Error + Send + Sync + 'static>),
}

impl HookResult {
  /// 是否应中断后续操作。
  pub fn should_abort_operation(&self) -> bool {
    matches!(self, Self::FailedAbort(_))
  }
}

// ── Hook 结构体 ───────────────────────────────────────────────────────────────

/// 单个 Hook，包含名称和执行函数。
#[derive(Clone)]
pub struct Hook {
  pub name: String,
  pub func: HookFn,
}

impl Default for Hook {
  fn default() -> Self {
    Self {
      name: "default".to_string(),
      func: Arc::new(|_| Box::pin(async { HookResult::Success })),
    }
  }
}

impl Hook {
  pub async fn execute(&self, payload: &HookPayload) -> HookResponse {
    HookResponse {
      hook_name: self.name.clone(),
      result: (self.func)(payload).await,
    }
  }
}

/// Hook 执行响应（名称 + 结果）。
#[derive(Debug)]
pub struct HookResponse {
  pub hook_name: String,
  pub result: HookResult,
}

// ── Payload 结构体 ────────────────────────────────────────────────────────────

/// 传递给 Hook 的完整上下文。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookPayload {
  /// 会话 ID。
  pub session_id: String,
  /// 当前工作目录。
  pub cwd: PathBuf,
  /// 触发时间（ISO 8601）。
  pub triggered_at: DateTime<Utc>,
  /// 具体事件信息。
  pub hook_event: HookEvent,
}

// ── 事件枚举 ──────────────────────────────────────────────────────────────────

/// Hook 事件类型，1:1 codex HookEvent 加 BeforeToolCall。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum HookEvent {
  /// 工具调用前触发（可阻断）。
  BeforeToolCall {
    #[serde(flatten)]
    event: HookEventBeforeToolCall,
  },
  /// 工具调用后触发（成功或失败）。
  AfterToolCall {
    #[serde(flatten)]
    event: HookEventAfterToolCall,
  },
  /// 完整 Turn 结束后触发。
  AfterTurn {
    #[serde(flatten)]
    event: HookEventAfterTurn,
  },
}

/// BeforeToolCall 事件字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEventBeforeToolCall {
  pub turn_id: String,
  pub call_id: String,
  pub tool_name: String,
  /// 工具参数（JSON 字符串）。
  pub tool_args: String,
}

/// AfterToolCall 事件字段，1:1 codex HookEventAfterToolUse。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEventAfterToolCall {
  pub turn_id: String,
  pub call_id: String,
  pub tool_name: String,
  /// 工具参数（JSON 字符串）。
  pub tool_args: String,
  /// 是否实际执行（可能因审批被拒绝而未执行）。
  pub executed: bool,
  /// 是否成功。
  pub success: bool,
  /// 执行耗时（毫秒）。
  pub duration_ms: u64,
  /// 是否为可变操作（写文件、执行命令等）。
  pub mutating: bool,
  /// 沙箱策略名称。
  pub sandbox_policy: String,
  /// 输出预览（截断的结果文本）。
  pub output_preview: String,
}

/// AfterTurn 事件字段，1:1 codex HookEventAfterAgent。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEventAfterTurn {
  pub thread_id: String,
  pub turn_id: String,
  /// 用户输入消息列表（文本）。
  pub input_messages: Vec<String>,
  /// 最后一条 Assistant 消息（如有）。
  pub last_assistant_message: Option<String>,
}

// ── BeforeToolCall 决策 ────────────────────────────────────────────────────────

/// BeforeToolCall hook 可返回的决策。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BeforeToolDecision {
  /// 允许工具执行（默认）。
  #[default]
  Allow,
  /// 阻断工具执行，并提供原因（返回给模型）。
  Block { reason: String },
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn hook_result_abort_should_abort() {
    let result = HookResult::FailedAbort(Box::new(std::io::Error::other("abort")));
    assert!(result.should_abort_operation());
  }

  #[test]
  fn hook_result_continue_should_not_abort() {
    let result = HookResult::FailedContinue(Box::new(std::io::Error::other("continue")));
    assert!(!result.should_abort_operation());
  }

  #[test]
  fn hook_result_success_should_not_abort() {
    assert!(!HookResult::Success.should_abort_operation());
  }

  #[test]
  fn hook_default_is_noop() {
    let hook = Hook::default();
    assert_eq!(hook.name, "default");
  }

  #[test]
  fn before_tool_decision_default_is_allow() {
    assert_eq!(BeforeToolDecision::default(), BeforeToolDecision::Allow);
  }

  #[tokio::test]
  async fn hook_execute_returns_response_with_name() {
    let hook = Hook {
      name: "test-hook".to_string(),
      func: Arc::new(|_| Box::pin(async { HookResult::Success })),
    };
    let payload = HookPayload {
      session_id: "sess-1".to_string(),
      cwd: PathBuf::from("/tmp"),
      triggered_at: Utc::now(),
      hook_event: HookEvent::AfterTurn {
        event: HookEventAfterTurn {
          thread_id: "t1".to_string(),
          turn_id: "turn-1".to_string(),
          input_messages: vec!["hello".to_string()],
          last_assistant_message: Some("hi".to_string()),
        },
      },
    };
    let response = hook.execute(&payload).await;
    assert_eq!(response.hook_name, "test-hook");
    assert!(matches!(response.result, HookResult::Success));
  }

  #[test]
  fn hook_event_serializes_with_event_type_tag() {
    let event = HookEvent::AfterToolCall {
      event: HookEventAfterToolCall {
        turn_id: "t1".to_string(),
        call_id: "c1".to_string(),
        tool_name: "edit_file".to_string(),
        tool_args: "{}".to_string(),
        executed: true,
        success: true,
        duration_ms: 42,
        mutating: true,
        sandbox_policy: "danger-full-access".to_string(),
        output_preview: "ok".to_string(),
      },
    };
    let val = serde_json::to_value(&event).unwrap();
    assert_eq!(val["event_type"], "after_tool_call");
    assert_eq!(val["tool_name"], "edit_file");
  }

  #[test]
  fn before_tool_call_event_serializes() {
    let event = HookEvent::BeforeToolCall {
      event: HookEventBeforeToolCall {
        turn_id: "t1".to_string(),
        call_id: "c1".to_string(),
        tool_name: "shell".to_string(),
        tool_args: r#"{"command":"ls"}"#.to_string(),
      },
    };
    let val = serde_json::to_value(&event).unwrap();
    assert_eq!(val["event_type"], "before_tool_call");
    assert_eq!(val["tool_name"], "shell");
  }
}
