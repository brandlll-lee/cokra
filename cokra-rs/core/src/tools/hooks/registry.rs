//! Hook 注册表 — 管理 before/after 工具调用和 turn 生命周期 hooks。
//!
//! 1:1 复刻 codex `hooks/src/registry.rs` 设计。

use crate::tools::hooks::config::HooksConfig;
use crate::tools::hooks::types::Hook;
use crate::tools::hooks::types::HookEvent;
use crate::tools::hooks::types::HookPayload;
use crate::tools::hooks::types::HookResponse;

/// Hook 注册表，按事件分组存储 hooks。
#[derive(Clone, Default)]
pub struct HooksRegistry {
  /// 工具调用前 hooks（按注册顺序执行）。
  before_tool_call: Vec<Hook>,
  /// 工具调用后 hooks（按注册顺序执行）。
  after_tool_call: Vec<Hook>,
  /// Turn 结束后 hooks（按注册顺序执行）。
  after_turn: Vec<Hook>,
}

impl HooksRegistry {
  /// 从配置构建注册表（注册外部命令 hooks）。
  pub fn from_config(config: &HooksConfig) -> Self {
    let mut registry = Self::default();

    for cmd_hook in &config.before_tool_call {
      registry.register_before_tool_call(crate::tools::hooks::runner::command_hook(
        &cmd_hook.name,
        &cmd_hook.command,
        cmd_hook.timeout_ms,
      ));
    }

    for cmd_hook in &config.after_tool_call {
      registry.register_after_tool_call(crate::tools::hooks::runner::command_hook(
        &cmd_hook.name,
        &cmd_hook.command,
        cmd_hook.timeout_ms,
      ));
    }

    for cmd_hook in &config.after_turn {
      registry.register_after_turn(crate::tools::hooks::runner::command_hook(
        &cmd_hook.name,
        &cmd_hook.command,
        cmd_hook.timeout_ms,
      ));
    }

    registry
  }

  // ── 注册方法 ─────────────────────────────────────────────────────────

  pub fn register_before_tool_call(&mut self, hook: Hook) {
    self.before_tool_call.push(hook);
  }

  pub fn register_after_tool_call(&mut self, hook: Hook) {
    self.after_tool_call.push(hook);
  }

  pub fn register_after_turn(&mut self, hook: Hook) {
    self.after_turn.push(hook);
  }

  // ── 查询方法 ─────────────────────────────────────────────────────────

  pub fn hooks_for_event<'a>(&'a self, event: &HookEvent) -> &'a [Hook] {
    match event {
      HookEvent::BeforeToolCall { .. } => &self.before_tool_call,
      HookEvent::AfterToolCall { .. } => &self.after_tool_call,
      HookEvent::AfterTurn { .. } => &self.after_turn,
    }
  }

  pub fn has_before_tool_call_hooks(&self) -> bool {
    !self.before_tool_call.is_empty()
  }

  pub fn has_after_tool_call_hooks(&self) -> bool {
    !self.after_tool_call.is_empty()
  }

  pub fn has_after_turn_hooks(&self) -> bool {
    !self.after_turn.is_empty()
  }

  /// 分发一个 payload 到对应事件的所有 hooks，按序执行。
  /// FailedAbort 时立即停止并返回当前结果集。
  pub async fn dispatch(&self, payload: HookPayload) -> Vec<HookResponse> {
    let hooks = self.hooks_for_event(&payload.hook_event);
    let mut outcomes = Vec::with_capacity(hooks.len());

    for hook in hooks {
      let outcome = hook.execute(&payload).await;
      let should_abort = outcome.result.should_abort_operation();
      outcomes.push(outcome);
      if should_abort {
        break;
      }
    }

    outcomes
  }
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;
  use std::sync::Arc;
  use std::sync::atomic::AtomicUsize;
  use std::sync::atomic::Ordering;

  use chrono::Utc;

  use super::*;
  use crate::tools::hooks::types::HookEvent;
  use crate::tools::hooks::types::HookEventAfterTurn;
  use crate::tools::hooks::types::HookPayload;
  use crate::tools::hooks::types::HookResult;

  fn make_payload(label: &str) -> HookPayload {
    HookPayload {
      session_id: format!("sess-{label}"),
      cwd: PathBuf::from("/tmp"),
      triggered_at: Utc::now(),
      hook_event: HookEvent::AfterTurn {
        event: HookEventAfterTurn {
          thread_id: format!("thread-{label}"),
          turn_id: format!("turn-{label}"),
          input_messages: vec!["hello".to_string()],
          last_assistant_message: Some("hi".to_string()),
        },
      },
    }
  }

  fn counting_hook(calls: &Arc<AtomicUsize>, name: &str) -> Hook {
    let calls = Arc::clone(calls);
    let name = name.to_string();
    Hook {
      name,
      func: Arc::new(move |_| {
        let calls = Arc::clone(&calls);
        Box::pin(async move {
          calls.fetch_add(1, Ordering::SeqCst);
          HookResult::Success
        })
      }),
    }
  }

  fn abort_hook(name: &str) -> Hook {
    let name = name.to_string();
    Hook {
      name,
      func: Arc::new(move |_| {
        Box::pin(async move { HookResult::FailedAbort(Box::new(std::io::Error::other("abort!"))) })
      }),
    }
  }

  #[test]
  fn empty_registry_has_no_hooks() {
    let registry = HooksRegistry::default();
    assert!(!registry.has_before_tool_call_hooks());
    assert!(!registry.has_after_tool_call_hooks());
    assert!(!registry.has_after_turn_hooks());
  }

  #[tokio::test]
  async fn dispatch_executes_after_turn_hooks() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = HooksRegistry::default();
    registry.register_after_turn(counting_hook(&calls, "h1"));
    registry.register_after_turn(counting_hook(&calls, "h2"));

    let outcomes = registry.dispatch(make_payload("1")).await;
    assert_eq!(outcomes.len(), 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
  }

  #[tokio::test]
  async fn dispatch_stops_on_abort() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = HooksRegistry::default();
    registry.register_after_turn(abort_hook("aborter"));
    registry.register_after_turn(counting_hook(&calls, "unreached"));

    let outcomes = registry.dispatch(make_payload("abort")).await;
    assert_eq!(outcomes.len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(outcomes[0].result.should_abort_operation());
  }

  #[tokio::test]
  async fn dispatch_continues_on_failed_continue() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = HooksRegistry::default();

    let name = "soft-fail";
    registry.register_after_turn(Hook {
      name: name.to_string(),
      func: Arc::new(|_| {
        Box::pin(
          async move { HookResult::FailedContinue(Box::new(std::io::Error::other("soft fail"))) },
        )
      }),
    });
    registry.register_after_turn(counting_hook(&calls, "after-soft-fail"));

    let outcomes = registry.dispatch(make_payload("continue")).await;
    assert_eq!(outcomes.len(), 2);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
  }

  #[tokio::test]
  async fn dispatch_before_tool_call_only_runs_before_hooks() {
    use crate::tools::hooks::types::HookEvent;
    use crate::tools::hooks::types::HookEventBeforeToolCall;

    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = HooksRegistry::default();
    registry.register_before_tool_call(counting_hook(&calls, "before"));
    registry.register_after_tool_call(counting_hook(&calls, "after"));

    let payload = HookPayload {
      session_id: "s".to_string(),
      cwd: PathBuf::from("/tmp"),
      triggered_at: Utc::now(),
      hook_event: HookEvent::BeforeToolCall {
        event: HookEventBeforeToolCall {
          turn_id: "t1".to_string(),
          call_id: "c1".to_string(),
          tool_name: "edit_file".to_string(),
          tool_args: "{}".to_string(),
        },
      },
    };

    let outcomes = registry.dispatch(payload).await;
    // 只有 before_tool_call hook 被触发
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].hook_name, "before");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
  }
}
