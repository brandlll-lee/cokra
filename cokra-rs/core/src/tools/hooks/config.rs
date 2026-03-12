//! Hooks 系统配置结构体。
//!
//! 对应 `.cokra/config.toml` 或 `~/.cokra/config.toml` 中的 `[hooks]` 段。
//!
//! 示例配置：
//! ```toml
//! [[hooks.after_tool_call]]
//! name = "notify"
//! command = "notify-send"
//! timeout_ms = 5000
//!
//! [[hooks.after_turn]]
//! name = "log-turn"
//! command = "/usr/local/bin/log-turn.sh"
//! ```

use serde::Deserialize;
use serde::Serialize;

/// 单个命令 hook 的配置条目。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandHookConfig {
  /// Hook 名称（用于日志和结果追踪）。
  pub name: String,
  /// 要执行的命令（argv[0]，可带参数）。
  pub command: String,
  /// 超时时间（毫秒，默认 10000）。
  #[serde(default = "default_timeout_ms")]
  pub timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
  10_000
}

/// `[hooks]` 配置段。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksConfig {
  /// 工具调用前触发的 hooks。
  #[serde(default)]
  pub before_tool_call: Vec<CommandHookConfig>,
  /// 工具调用后触发的 hooks。
  #[serde(default)]
  pub after_tool_call: Vec<CommandHookConfig>,
  /// Turn 完成后触发的 hooks。
  #[serde(default)]
  pub after_turn: Vec<CommandHookConfig>,
}

impl HooksConfig {
  pub fn is_empty(&self) -> bool {
    self.before_tool_call.is_empty()
      && self.after_tool_call.is_empty()
      && self.after_turn.is_empty()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn empty_config_is_empty() {
    let config = HooksConfig::default();
    assert!(config.is_empty());
  }

  #[test]
  fn config_with_hook_is_not_empty() {
    let config = HooksConfig {
      after_turn: vec![CommandHookConfig {
        name: "notify".to_string(),
        command: "notify-send".to_string(),
        timeout_ms: 5000,
      }],
      ..Default::default()
    };
    assert!(!config.is_empty());
  }

  #[test]
  fn command_hook_config_deserializes_with_defaults() {
    let toml_str = r#"
      name = "my-hook"
      command = "/usr/local/bin/hook.sh"
    "#;
    let cfg: CommandHookConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.name, "my-hook");
    assert_eq!(cfg.command, "/usr/local/bin/hook.sh");
    assert_eq!(cfg.timeout_ms, 10_000);
  }

  #[test]
  fn command_hook_config_deserializes_with_explicit_timeout() {
    let toml_str = r#"
      name = "fast-hook"
      command = "echo"
      timeout_ms = 2000
    "#;
    let cfg: CommandHookConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.timeout_ms, 2000);
  }

  #[test]
  fn hooks_config_deserializes_from_toml() {
    let toml_str = r#"
      [[before_tool_call]]
      name = "pre-check"
      command = "/tmp/pre-check.sh"

      [[after_tool_call]]
      name = "post-log"
      command = "logger"
      timeout_ms = 3000

      [[after_turn]]
      name = "notify"
      command = "notify-send"
    "#;
    let cfg: HooksConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.before_tool_call.len(), 1);
    assert_eq!(cfg.after_tool_call.len(), 1);
    assert_eq!(cfg.after_turn.len(), 1);
    assert_eq!(cfg.before_tool_call[0].name, "pre-check");
    assert_eq!(cfg.after_tool_call[0].timeout_ms, 3000);
    assert_eq!(cfg.after_turn[0].command, "notify-send");
  }

  #[test]
  fn hooks_config_serializes_to_json() {
    let config = HooksConfig {
      before_tool_call: vec![],
      after_tool_call: vec![CommandHookConfig {
        name: "n".to_string(),
        command: "c".to_string(),
        timeout_ms: 1000,
      }],
      after_turn: vec![],
    };
    let json = serde_json::to_string(&config).unwrap();
    assert!(json.contains("after_tool_call"));
    assert!(json.contains("\"name\":\"n\""));
  }
}
