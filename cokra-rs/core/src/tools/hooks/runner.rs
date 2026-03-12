//! Hook 执行器 — 外部命令 hook 工厂函数。
//!
//! 1:1 复刻 codex `hooks/src/registry.rs::command_from_argv` 模式：
//! - 将 HookPayload 序列化为 JSON
//! - 通过 stdin 传递给子进程（允许大 payload）
//! - 支持超时（默认 10 秒）
//! - 非零退出码 → FailedContinue（不中断工具调用）

use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::tools::hooks::types::Hook;
use crate::tools::hooks::types::HookPayload;
use crate::tools::hooks::types::HookResult;

const DEFAULT_TIMEOUT_MS: u64 = 10_000;

/// 构建一个外部命令 hook。
///
/// 命令通过 shell 执行：`sh -c "<command>"` (Unix) / `cmd /C "<command>"` (Windows)。
/// HookPayload 序列化为 JSON 后通过 stdin 传入子进程。
///
/// - 退出码 0 → `HookResult::Success`
/// - 退出码 2 → `HookResult::FailedAbort`（约定：2 = 主动阻断）
/// - 其他非零 → `HookResult::FailedContinue`
/// - 超时 → `HookResult::FailedContinue`（记录警告，不中断）
pub fn command_hook(name: &str, command: &str, timeout_ms: u64) -> Hook {
  let name_owned = name.to_string();
  let command_owned = command.to_string();
  let effective_timeout = if timeout_ms == 0 {
    DEFAULT_TIMEOUT_MS
  } else {
    timeout_ms
  };

  Hook {
    name: name_owned.clone(),
    func: std::sync::Arc::new(move |payload: &HookPayload| {
      let name_clone = name_owned.clone();
      let command_clone = command_owned.clone();
      let payload_clone = payload.clone();

      Box::pin(async move {
        run_command_hook(
          &name_clone,
          &command_clone,
          &payload_clone,
          effective_timeout,
        )
        .await
      })
    }),
  }
}

async fn run_command_hook(
  hook_name: &str,
  command: &str,
  payload: &HookPayload,
  timeout_ms: u64,
) -> HookResult {
  let json = match serde_json::to_string(payload) {
    Ok(j) => j,
    Err(e) => {
      return HookResult::FailedContinue(Box::new(std::io::Error::other(format!(
        "hook '{hook_name}': payload 序列化失败: {e}"
      ))));
    }
  };

  let mut child = match spawn_command(command) {
    Ok(c) => c,
    Err(e) => {
      return HookResult::FailedContinue(Box::new(std::io::Error::other(format!(
        "hook '{hook_name}': 启动命令失败 `{command}`: {e}"
      ))));
    }
  };

  // 通过 stdin 传递 payload JSON
  if let Some(mut stdin) = child.stdin.take() {
    let _ = stdin.write_all(json.as_bytes()).await;
    // stdin 关闭让子进程感知 EOF
    drop(stdin);
  }

  let deadline = Duration::from_millis(timeout_ms);
  match timeout(deadline, child.wait()).await {
    Ok(Ok(status)) => {
      let code = status.code().unwrap_or(-1);
      if code == 0 {
        HookResult::Success
      } else if code == 2 {
        // 约定：退出码 2 = 主动中断（FailedAbort）
        HookResult::FailedAbort(Box::new(std::io::Error::other(format!(
          "hook '{hook_name}': 以退出码 2 中断操作"
        ))))
      } else {
        HookResult::FailedContinue(Box::new(std::io::Error::other(format!(
          "hook '{hook_name}': 命令以退出码 {code} 退出"
        ))))
      }
    }
    Ok(Err(e)) => HookResult::FailedContinue(Box::new(std::io::Error::other(format!(
      "hook '{hook_name}': wait() 失败: {e}"
    )))),
    Err(_) => {
      // 超时：尝试 kill 子进程
      let _ = child.kill().await;
      HookResult::FailedContinue(Box::new(std::io::Error::other(format!(
        "hook '{hook_name}': 超时（{timeout_ms}ms），进程已终止"
      ))))
    }
  }
}

/// 跨平台启动命令。
///
/// Unix:    `sh -c "<command>"`
/// Windows: `cmd /C "<command>"`
fn spawn_command(command: &str) -> std::io::Result<tokio::process::Child> {
  #[cfg(windows)]
  {
    Command::new("cmd")
      .args(["/C", command])
      .stdin(std::process::Stdio::piped())
      .stdout(std::process::Stdio::null())
      .stderr(std::process::Stdio::null())
      .spawn()
  }
  #[cfg(not(windows))]
  {
    Command::new("sh")
      .args(["-c", command])
      .stdin(std::process::Stdio::piped())
      .stdout(std::process::Stdio::null())
      .stderr(std::process::Stdio::null())
      .spawn()
  }
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use chrono::Utc;

  use super::*;
  use crate::tools::hooks::types::HookEvent;
  use crate::tools::hooks::types::HookEventAfterTurn;
  use crate::tools::hooks::types::HookPayload;

  fn test_payload() -> HookPayload {
    HookPayload {
      session_id: "test-session".to_string(),
      cwd: PathBuf::from("/tmp"),
      triggered_at: Utc::now(),
      hook_event: HookEvent::AfterTurn {
        event: HookEventAfterTurn {
          thread_id: "t1".to_string(),
          turn_id: "turn-1".to_string(),
          input_messages: vec!["hello".to_string()],
          last_assistant_message: Some("world".to_string()),
        },
      },
    }
  }

  #[tokio::test]
  async fn command_hook_success_on_exit_zero() {
    #[cfg(windows)]
    let cmd = "exit 0";
    #[cfg(not(windows))]
    let cmd = "exit 0";

    let hook = command_hook("test", cmd, 5000);
    let result = hook.execute(&test_payload()).await;
    assert!(matches!(result.result, HookResult::Success));
  }

  #[tokio::test]
  async fn command_hook_failed_continue_on_nonzero_exit() {
    #[cfg(windows)]
    let cmd = "exit 1";
    #[cfg(not(windows))]
    let cmd = "exit 1";

    let hook = command_hook("test-fail", cmd, 5000);
    let result = hook.execute(&test_payload()).await;
    assert!(matches!(result.result, HookResult::FailedContinue(_)));
    assert!(!result.result.should_abort_operation());
  }

  #[tokio::test]
  async fn command_hook_abort_on_exit_2() {
    #[cfg(windows)]
    let cmd = "exit 2";
    #[cfg(not(windows))]
    let cmd = "exit 2";

    let hook = command_hook("test-abort", cmd, 5000);
    let result = hook.execute(&test_payload()).await;
    assert!(result.result.should_abort_operation());
  }

  #[tokio::test]
  async fn command_hook_failed_continue_on_timeout() {
    #[cfg(windows)]
    let cmd = "timeout /t 10 /nobreak";
    #[cfg(not(windows))]
    let cmd = "sleep 10";

    // 设置 50ms 超时，sleep 10s 必然超时
    let hook = command_hook("timeout-hook", cmd, 50);
    let result = hook.execute(&test_payload()).await;
    assert!(matches!(result.result, HookResult::FailedContinue(_)));
    assert!(!result.result.should_abort_operation());
  }

  #[tokio::test]
  async fn command_hook_failed_continue_on_nonexistent_command() {
    let hook = command_hook("missing", "__nonexistent_cokra_hook_binary_xyz__", 5000);
    let result = hook.execute(&test_payload()).await;
    // 命令不存在：spawn 失败 → FailedContinue
    assert!(matches!(result.result, HookResult::FailedContinue(_)));
  }

  #[tokio::test]
  async fn command_hook_name_preserved() {
    let hook = command_hook("my-hook-name", "exit 0", 1000);
    assert_eq!(hook.name, "my-hook-name");
  }

  #[tokio::test]
  async fn command_hook_receives_json_payload_via_stdin() {
    // 验证 hook 能从 stdin 接收 JSON payload（通过 cat 读取并验证非空）
    #[cfg(not(windows))]
    {
      let payload = test_payload();
      // 读取 stdin 并验证是否包含 session_id 字段
      let cmd = r#"read -r line; echo "$line" | grep -q '"session_id"' && exit 0 || exit 1"#;
      let hook = command_hook("stdin-test", cmd, 5000);
      let result = hook.execute(&payload).await;
      assert!(matches!(result.result, HookResult::Success));
    }
    #[cfg(windows)]
    {
      // Windows: 简单 exit 0
      let hook = command_hook("stdin-test", "exit 0", 1000);
      let result = hook.execute(&test_payload()).await;
      assert!(matches!(result.result, HookResult::Success));
    }
  }
}
