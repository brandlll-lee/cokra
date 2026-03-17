//! todo_write 工具 handler。
//!
//! 1:1 复刻 opencode `session/todo.ts` + `tool/todo.ts` 设计：
//!
//! ## 设计原则
//! - Todo 列表按 session（thread_id）隔离存储：`~/.cokra/todos/{thread_id}.json`
//! - `todo_write` 接收完整列表并全量覆写，返回 JSON（含未完成任务数）
//! - `todo_read` 已废弃（1:1 opencode registry.ts:110 `// TodoReadTool`）
//!   — `todo_write` 的返回值本身就是完整列表，无需单独读取
//! - 状态枚举：pending / in_progress / completed / cancelled
//! - 约束：同时最多 1 个 in_progress 任务
//! - 优先级枚举：high / medium / low（可选，默认 medium）
//! - 字段名：`content`（1:1 opencode Todo.Info，兼容旧 `description` 别名）

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

// ── 数据结构 ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
  Pending,
  InProgress,
  Completed,
  Cancelled,
}

impl TodoStatus {
  pub fn as_str(&self) -> &'static str {
    match self {
      TodoStatus::Pending => "pending",
      TodoStatus::InProgress => "in_progress",
      TodoStatus::Completed => "completed",
      TodoStatus::Cancelled => "cancelled",
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TodoPriority {
  High,
  #[default]
  Medium,
  Low,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
  /// 唯一标识符（由调用者提供，字符串）。
  pub id: String,
  /// 任务内容描述。
  /// 1:1 opencode: 字段名为 `content`，同时接受 `description` 作为别名。
  #[serde(alias = "description")]
  pub content: String,
  /// 任务状态。
  pub status: TodoStatus,
  /// 任务优先级（可选，默认 medium）。
  #[serde(default)]
  pub priority: TodoPriority,
}

// ── todo_write handler ───────────────────────────────────────────────────────

pub struct TodoWriteHandler;

#[derive(Debug, Deserialize)]
struct TodoWriteArgs {
  /// 完整的 todo 列表（全量覆写）。
  todos: Vec<TodoItem>,
}

#[async_trait]
impl ToolHandler for TodoWriteHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn is_mutating(&self, _: &ToolInvocation) -> bool {
    true
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let id = invocation.id.clone();
    let args: TodoWriteArgs = invocation.parse_arguments()?;

    validate_todos(&args.todos)?;

    // 1:1 opencode: 按 thread_id（session）隔离存储
    let thread_id = invocation
      .runtime
      .as_ref()
      .map(|r| r.thread_id.as_str())
      .unwrap_or("default");
    let todo_path = todo_session_path(thread_id)
      .map_err(|e| FunctionCallError::Execution(format!("无法确定 todo 文件路径: {e}")))?;

    if let Some(parent) = todo_path.parent() {
      tokio::fs::create_dir_all(parent).await.map_err(|e| {
        FunctionCallError::Execution(format!("创建 todo 目录失败 {}: {e}", parent.display()))
      })?;
    }

    let json = serde_json::to_string_pretty(&args.todos)
      .map_err(|e| FunctionCallError::Execution(format!("序列化 todo 失败: {e}")))?;

    tokio::fs::write(&todo_path, &json)
      .await
      .map_err(|e| FunctionCallError::Execution(format!("写入 todo 文件失败: {e}")))?;

    // Emit TodoUpdate event to TUI
    if let Some(rt) = invocation.runtime.as_ref()
      && let Some(tx) = rt.tx_event.as_ref()
    {
      let event_todos: Vec<cokra_protocol::TodoItemEvent> = args
        .todos
        .iter()
        .map(|t| cokra_protocol::TodoItemEvent {
          id: t.id.clone(),
          content: t.content.clone(),
          status: match t.status {
            TodoStatus::Pending => cokra_protocol::TodoItemStatus::Pending,
            TodoStatus::InProgress => cokra_protocol::TodoItemStatus::InProgress,
            TodoStatus::Completed => cokra_protocol::TodoItemStatus::Completed,
            TodoStatus::Cancelled => cokra_protocol::TodoItemStatus::Cancelled,
          },
          priority: Some(match t.priority {
            TodoPriority::High => cokra_protocol::TodoItemPriority::High,
            TodoPriority::Medium => cokra_protocol::TodoItemPriority::Medium,
            TodoPriority::Low => cokra_protocol::TodoItemPriority::Low,
          }),
        })
        .collect();
      let _ = tx
        .send(cokra_protocol::EventMsg::TodoUpdate(
          cokra_protocol::TodoUpdateEvent {
            thread_id: rt.thread_id.clone(),
            todos: event_todos,
          },
        ))
        .await;
    }

    // 1:1 opencode: 返回完整 JSON 列表
    let output = serde_json::to_string_pretty(&args.todos).unwrap_or_else(|_| "[]".to_string());
    Ok(ToolOutput::success(output).with_id(id))
  }
}

// ── 文件路径 ─────────────────────────────────────────────────────────────────

/// 1:1 opencode `Storage.write(["todo", sessionID], todos)`:
/// 返回 `~/.cokra/todos/{thread_id}.json`。
pub fn todo_session_path(thread_id: &str) -> Result<PathBuf, String> {
  // sanitize thread_id: 只保留 alphanumeric、hyphen、underscore
  let safe_id: String = thread_id
    .chars()
    .map(|c| {
      if c.is_alphanumeric() || c == '-' || c == '_' {
        c
      } else {
        '_'
      }
    })
    .collect();
  let safe_id = if safe_id.is_empty() {
    "default".to_string()
  } else {
    safe_id
  };
  dirs::home_dir()
    .ok_or_else(|| "无法确定 home 目录".to_string())
    .map(|home| {
      home
        .join(".cokra")
        .join("todos")
        .join(format!("{safe_id}.json"))
    })
}

/// 向后兼容：按 thread_id 加载该 session 的 todos。
pub async fn load_session_todos(thread_id: &str) -> Result<Vec<TodoItem>, FunctionCallError> {
  let path = todo_session_path(thread_id)
    .map_err(|e| FunctionCallError::Execution(format!("无法确定 todo 文件路径: {e}")))?;

  match tokio::fs::read_to_string(&path).await {
    Ok(content) => {
      if content.trim().is_empty() {
        return Ok(Vec::new());
      }
      serde_json::from_str::<Vec<TodoItem>>(&content)
        .map_err(|e| FunctionCallError::Execution(format!("todo 文件 JSON 格式错误: {e}")))
    }
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
    Err(e) => Err(FunctionCallError::Execution(format!(
      "读取 todo 文件失败: {e}"
    ))),
  }
}

/// 校验 todo 列表的业务约束。
fn validate_todos(todos: &[TodoItem]) -> Result<(), FunctionCallError> {
  for todo in todos {
    if todo.id.trim().is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "每个 todo 的 id 不能为空".to_string(),
      ));
    }
    if todo.content.trim().is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "每个 todo 的 content 不能为空".to_string(),
      ));
    }
  }

  // 1:1 gemini-cli: 同时最多 1 个 in_progress
  let in_progress_count = todos
    .iter()
    .filter(|t| t.status == TodoStatus::InProgress)
    .count();

  if in_progress_count > 1 {
    return Err(FunctionCallError::RespondToModel(format!(
      "同时只能有 1 个 in_progress 任务，当前有 {in_progress_count} 个"
    )));
  }

  Ok(())
}

/// 将 todo 列表格式化为模型可读的文字摘要。
fn format_todos_for_model(todos: &[TodoItem]) -> String {
  if todos.is_empty() {
    return "Todo 列表为空。".to_string();
  }

  let mut lines = vec!["当前 Todo 列表:".to_string(), String::new()];

  // 按状态分组显示：in_progress → pending → completed → cancelled
  let order = [
    TodoStatus::InProgress,
    TodoStatus::Pending,
    TodoStatus::Completed,
    TodoStatus::Cancelled,
  ];

  for status in &order {
    let group: Vec<&TodoItem> = todos.iter().filter(|t| &t.status == status).collect();
    if group.is_empty() {
      continue;
    }

    let header = match status {
      TodoStatus::InProgress => "🔄 进行中",
      TodoStatus::Pending => "⏳ 待处理",
      TodoStatus::Completed => "✅ 已完成",
      TodoStatus::Cancelled => "❌ 已取消",
    };
    lines.push(format!("{header}:"));

    for item in group {
      let priority_badge = match item.priority {
        TodoPriority::High => "[高]",
        TodoPriority::Medium => "[中]",
        TodoPriority::Low => "[低]",
      };
      let id = &item.id;
      let status = item.status.as_str();
      let content = &item.content;
      lines.push(format!("  - [{id}] {priority_badge} {status} {content}"));
    }
    lines.push(String::new());
  }

  lines.join("\n").trim_end().to_string()
}

#[cfg(test)]
mod tests {
  use super::*;

  // ── validate_todos 测试 ───────────────────────────────────────────────

  #[test]
  fn validate_empty_list_passes() {
    assert!(validate_todos(&[]).is_ok());
  }

  #[test]
  fn validate_single_in_progress_passes() {
    let todos = vec![TodoItem {
      id: "1".to_string(),
      content: "Do something".to_string(),
      status: TodoStatus::InProgress,
      priority: TodoPriority::High,
    }];
    assert!(validate_todos(&todos).is_ok());
  }

  #[test]
  fn validate_two_in_progress_fails() {
    let todos = vec![
      TodoItem {
        id: "1".to_string(),
        content: "Task A".to_string(),
        status: TodoStatus::InProgress,
        priority: TodoPriority::Medium,
      },
      TodoItem {
        id: "2".to_string(),
        content: "Task B".to_string(),
        status: TodoStatus::InProgress,
        priority: TodoPriority::Medium,
      },
    ];
    let result = validate_todos(&todos);
    assert!(result.is_err());
    if let Err(FunctionCallError::RespondToModel(msg)) = result {
      assert!(msg.contains("in_progress"));
    }
  }

  #[test]
  fn validate_empty_id_fails() {
    let todos = vec![TodoItem {
      id: "  ".to_string(),
      content: "Task".to_string(),
      status: TodoStatus::Pending,
      priority: TodoPriority::Low,
    }];
    let result = validate_todos(&todos);
    assert!(result.is_err());
  }

  #[test]
  fn validate_empty_content_fails() {
    let todos = vec![TodoItem {
      id: "1".to_string(),
      content: "".to_string(),
      status: TodoStatus::Pending,
      priority: TodoPriority::Low,
    }];
    let result = validate_todos(&todos);
    assert!(result.is_err());
  }

  #[test]
  fn validate_mixed_statuses_passes() {
    let todos = vec![
      TodoItem {
        id: "1".to_string(),
        content: "In progress task".to_string(),
        status: TodoStatus::InProgress,
        priority: TodoPriority::High,
      },
      TodoItem {
        id: "2".to_string(),
        content: "Pending task".to_string(),
        status: TodoStatus::Pending,
        priority: TodoPriority::Medium,
      },
      TodoItem {
        id: "3".to_string(),
        content: "Done task".to_string(),
        status: TodoStatus::Completed,
        priority: TodoPriority::Low,
      },
    ];
    assert!(validate_todos(&todos).is_ok());
  }

  // ── format_todos_for_model 测试 ───────────────────────────────────────

  #[test]
  fn format_empty_todos() {
    let output = format_todos_for_model(&[]);
    assert!(output.contains("为空"));
  }

  #[test]
  fn format_todos_groups_by_status() {
    let todos = vec![
      TodoItem {
        id: "a".to_string(),
        content: "Active work".to_string(),
        status: TodoStatus::InProgress,
        priority: TodoPriority::High,
      },
      TodoItem {
        id: "b".to_string(),
        content: "Waiting".to_string(),
        status: TodoStatus::Pending,
        priority: TodoPriority::Low,
      },
    ];
    let output = format_todos_for_model(&todos);
    assert!(output.contains("进行中") || output.contains("🔄"));
    assert!(output.contains("待处理") || output.contains("⏳"));
    // in_progress 在 pending 之前
    let in_progress_pos = output.find("Active work").unwrap_or(usize::MAX);
    let pending_pos = output.find("Waiting").unwrap_or(usize::MAX);
    assert!(in_progress_pos < pending_pos);
  }

  // ── load/save 集成测试 ────────────────────────────────────────────────

  #[tokio::test]
  async fn todo_write_and_read_roundtrip() {
    // 临时重定向 home 到 tempdir（通过 invocation cwd）
    // 我们直接测试 load/save 内部逻辑（使用实际 home）
    // 注意：这只验证序列化/反序列化，不验证真实文件系统副作用

    let todos = vec![
      TodoItem {
        id: "t1".to_string(),
        content: "Write tests".to_string(),
        status: TodoStatus::InProgress,
        priority: TodoPriority::High,
      },
      TodoItem {
        id: "t2".to_string(),
        content: "Review PR".to_string(),
        status: TodoStatus::Pending,
        priority: TodoPriority::Medium,
      },
    ];

    // 序列化再反序列化
    let json = serde_json::to_string_pretty(&todos).unwrap();
    let loaded: Vec<TodoItem> = serde_json::from_str(&json).unwrap();

    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].id, "t1");
    assert_eq!(loaded[0].status, TodoStatus::InProgress);
    assert_eq!(loaded[1].id, "t2");
    assert_eq!(loaded[1].status, TodoStatus::Pending);
  }

  #[tokio::test]
  async fn todo_write_handler_rejects_two_in_progress() {
    use serde_json::json;

    let dir = tempfile::tempdir().unwrap();
    let invocation = crate::tools::context::ToolInvocation {
      id: "tw-1".to_string(),
      name: "todo_write".to_string(),
      payload: crate::tools::context::ToolPayload::Function {
        arguments: json!({
          "todos": [
            { "id": "1", "content": "A", "status": "in_progress", "priority": "high" },
            { "id": "2", "content": "B", "status": "in_progress", "priority": "medium" }
          ]
        })
        .to_string(),
      },
      cwd: dir.path().to_path_buf(),
      runtime: None,
    };

    let handler = TodoWriteHandler;
    let result = handler.handle_async(invocation).await;
    assert!(result.is_err());
  }

  #[tokio::test]
  async fn load_session_todos_returns_empty_when_no_file() {
    // 1:1 opencode: session 隔离，随机 thread_id 不存在时返回空列表
    let result = load_session_todos("nonexistent-session-xyz").await;
    assert!(result.is_ok(), "文件不存在应返回空列表，而非错误");
    assert!(result.unwrap().is_empty());
  }

  #[tokio::test]
  async fn todo_session_path_sanitizes_thread_id() {
    // thread_id 中的特殊字符应被替换为 _
    let path = todo_session_path("my/session:test").unwrap();
    let filename = path.file_name().unwrap().to_str().unwrap();
    assert!(!filename.contains('/'), "路径分隔符应被清理");
    assert!(!filename.contains(':'), "冒号应被清理");
    assert!(filename.ends_with(".json"));
  }

  #[test]
  fn todo_session_paths_are_isolated_by_thread_id() {
    // 1:1 opencode: 不同 session 的路径应完全不同
    let path_a = todo_session_path("session-alpha").unwrap();
    let path_b = todo_session_path("session-beta").unwrap();
    assert_ne!(path_a, path_b, "不同 session 应有不同存储路径");
    assert!(path_a.to_str().unwrap().contains("session-alpha"));
    assert!(path_b.to_str().unwrap().contains("session-beta"));
    // 两者都在同一 todos/ 目录下
    assert_eq!(path_a.parent(), path_b.parent());
  }
}
