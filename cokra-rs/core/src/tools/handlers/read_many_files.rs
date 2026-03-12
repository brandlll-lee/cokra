//! read_many_files 工具 handler — 批量读取多个文件内容。
//!
//! 复刻 opencode `tool/read.ts` 中的批量读取模式 + codex `read_file` handler 的行限制策略。
//!
//! ## 设计原则
//! - 最多一次读取 20 个文件（防止上下文爆炸）
//! - 每个文件最多 2000 行（与 read_file 对齐）
//! - 每行最多 500 字节（与 read_file 对齐）
//! - 路径必须为绝对路径
//! - 读取失败的文件单独报错，不中断整批

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ReadManyFilesHandler;

const MAX_FILES: usize = 20;
const MAX_LINES_PER_FILE: usize = 2000;
const MAX_LINE_BYTES: usize = 500;

#[derive(Debug, Deserialize)]
struct ReadManyFilesArgs {
  /// 要读取的绝对路径列表，最多 20 个。
  paths: Vec<String>,
  /// 每个文件从第几行开始（1-indexed，默认 1）。
  #[serde(default = "default_offset")]
  offset: usize,
  /// 每个文件最多读取多少行（默认 2000）。
  #[serde(default = "default_limit")]
  limit: usize,
}

fn default_offset() -> usize {
  1
}

fn default_limit() -> usize {
  MAX_LINES_PER_FILE
}

#[async_trait]
impl ToolHandler for ReadManyFilesHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let id = invocation.id.clone();
    let args: ReadManyFilesArgs = invocation.parse_arguments()?;

    if args.paths.is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "paths 不能为空".to_string(),
      ));
    }

    if args.paths.len() > MAX_FILES {
      return Err(FunctionCallError::RespondToModel(format!(
        "一次最多读取 {MAX_FILES} 个文件，当前请求了 {} 个",
        args.paths.len()
      )));
    }

    if args.offset == 0 {
      return Err(FunctionCallError::RespondToModel(
        "offset 必须是 1-indexed 行号（≥1）".to_string(),
      ));
    }

    if args.limit == 0 {
      return Err(FunctionCallError::RespondToModel(
        "limit 必须大于 0".to_string(),
      ));
    }

    let effective_limit = args.limit.min(MAX_LINES_PER_FILE);

    // 并发读取所有文件
    let tasks: Vec<_> = args
      .paths
      .iter()
      .map(|path_str| {
        let path = PathBuf::from(path_str);
        let path_str = path_str.clone();
        let offset = args.offset;
        let limit = effective_limit;
        tokio::spawn(async move { read_one_file(path_str, path, offset, limit).await })
      })
      .collect();

    let mut sections = Vec::with_capacity(tasks.len());
    for task in tasks {
      match task.await {
        Ok(section) => sections.push(section),
        Err(e) => sections.push(format!("<!-- join error: {e} -->")),
      }
    }

    let output = sections.join("\n\n");
    Ok(ToolOutput::success(output).with_id(id))
  }
}

/// 读取单个文件，返回带文件头注释的内容块。
async fn read_one_file(path_str: String, path: PathBuf, offset: usize, limit: usize) -> String {
  if !path.is_absolute() {
    return format!("=== {path_str} ===\nError: 路径必须为绝对路径，收到: {path_str}");
  }

  match read_slice_async(&path, offset, limit).await {
    Ok(lines) => {
      if lines.is_empty() {
        format!("=== {path_str} ===\n(空文件或 offset 超出文件长度)")
      } else {
        format!("=== {path_str} ===\n{}", lines.join("\n"))
      }
    }
    Err(e) => format!("=== {path_str} ===\nError: {e}"),
  }
}

/// 读取文件的指定行范围，每行格式为 `L{n}: {content}`（与 read_file 对齐）。
async fn read_slice_async(
  path: &std::path::Path,
  offset: usize,
  limit: usize,
) -> Result<Vec<String>, String> {
  use tokio::io::AsyncBufReadExt;

  let file = tokio::fs::File::open(path)
    .await
    .map_err(|e| format!("打开文件失败: {e}"))?;

  let mut reader = tokio::io::BufReader::new(file);
  let mut collected = Vec::new();
  let mut seen: usize = 0;
  let mut buffer = Vec::new();

  loop {
    buffer.clear();
    let bytes_read = reader
      .read_until(b'\n', &mut buffer)
      .await
      .map_err(|e| format!("读取文件失败: {e}"))?;

    if bytes_read == 0 {
      break;
    }

    // 去除行尾 \n 和 \r\n
    if buffer.last() == Some(&b'\n') {
      buffer.pop();
      if buffer.last() == Some(&b'\r') {
        buffer.pop();
      }
    }

    seen += 1;

    if seen < offset {
      continue;
    }

    if collected.len() >= limit {
      break;
    }

    let line_str = String::from_utf8_lossy(&buffer);
    let truncated = if line_str.len() > MAX_LINE_BYTES {
      // 在字符边界截断
      let mut end = MAX_LINE_BYTES;
      while end > 0 && !line_str.is_char_boundary(end) {
        end -= 1;
      }
      format!("{}…", &line_str[..end])
    } else {
      line_str.into_owned()
    };

    collected.push(format!("L{seen}: {truncated}"));
  }

  Ok(collected)
}

#[cfg(test)]
mod tests {
  use super::*;

  // ── read_slice_async 测试 ─────────────────────────────────────────────

  #[tokio::test]
  async fn read_slice_reads_all_lines() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    tokio::fs::write(&file, "line1\nline2\nline3\n")
      .await
      .unwrap();

    let lines = read_slice_async(&file, 1, 100).await.unwrap();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "L1: line1");
    assert_eq!(lines[1], "L2: line2");
    assert_eq!(lines[2], "L3: line3");
  }

  #[tokio::test]
  async fn read_slice_respects_offset() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    tokio::fs::write(&file, "a\nb\nc\nd\n").await.unwrap();

    let lines = read_slice_async(&file, 3, 10).await.unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "L3: c");
    assert_eq!(lines[1], "L4: d");
  }

  #[tokio::test]
  async fn read_slice_respects_limit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    tokio::fs::write(&file, "a\nb\nc\nd\ne\n").await.unwrap();

    let lines = read_slice_async(&file, 1, 3).await.unwrap();
    assert_eq!(lines.len(), 3);
  }

  #[tokio::test]
  async fn read_slice_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("empty.txt");
    tokio::fs::write(&file, "").await.unwrap();

    let lines = read_slice_async(&file, 1, 100).await.unwrap();
    assert!(lines.is_empty());
  }

  #[tokio::test]
  async fn read_slice_nonexistent_file_returns_err() {
    let result = read_slice_async(std::path::Path::new("/nonexistent/file.txt"), 1, 10).await;
    assert!(result.is_err());
  }

  // ── read_one_file 测试 ────────────────────────────────────────────────

  #[tokio::test]
  async fn read_one_file_includes_header() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("hello.rs");
    tokio::fs::write(&file, "fn main() {}\n").await.unwrap();

    let path_str = file.to_string_lossy().to_string();
    let section = read_one_file(path_str.clone(), file, 1, 100).await;
    assert!(section.starts_with(&format!("=== {path_str} ===")));
    assert!(section.contains("fn main()"));
  }

  #[tokio::test]
  async fn read_one_file_relative_path_returns_error() {
    let section = read_one_file(
      "relative/path.txt".to_string(),
      PathBuf::from("relative/path.txt"),
      1,
      10,
    )
    .await;
    assert!(section.contains("Error:"));
    assert!(section.contains("绝对路径"));
  }

  #[tokio::test]
  async fn read_one_file_missing_file_returns_error_section() {
    let path = "/absolutely/missing/file.txt";
    let section = read_one_file(path.to_string(), PathBuf::from(path), 1, 10).await;
    assert!(section.contains("Error:"));
  }

  // ── handler 集成测试 ──────────────────────────────────────────────────

  #[tokio::test]
  async fn handler_reads_multiple_files() {
    use serde_json::json;

    let dir = tempfile::tempdir().unwrap();
    let f1 = dir.path().join("a.txt");
    let f2 = dir.path().join("b.txt");
    tokio::fs::write(&f1, "alpha\nbeta\n").await.unwrap();
    tokio::fs::write(&f2, "gamma\ndelta\n").await.unwrap();

    let paths_val = json!([f1.to_string_lossy(), f2.to_string_lossy()]);
    let invocation = crate::tools::context::ToolInvocation {
      id: "test-1".to_string(),
      name: "read_many_files".to_string(),
      payload: crate::tools::context::ToolPayload::Function {
        arguments: json!({ "paths": paths_val }).to_string(),
      },
      cwd: dir.path().to_path_buf(),
      runtime: None,
    };

    let handler = ReadManyFilesHandler;
    let output = handler.handle_async(invocation).await.unwrap();
    let text = output.text_content();
    assert!(text.contains("alpha"));
    assert!(text.contains("gamma"));
  }

  #[tokio::test]
  async fn handler_rejects_too_many_paths() {
    use serde_json::json;

    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<String> = (0..=MAX_FILES)
      .map(|i| format!("/tmp/file{i}.txt"))
      .collect();

    let invocation = crate::tools::context::ToolInvocation {
      id: "test-2".to_string(),
      name: "read_many_files".to_string(),
      payload: crate::tools::context::ToolPayload::Function {
        arguments: json!({ "paths": paths }).to_string(),
      },
      cwd: dir.path().to_path_buf(),
      runtime: None,
    };

    let handler = ReadManyFilesHandler;
    let result = handler.handle_async(invocation).await;
    assert!(result.is_err());
    if let Err(FunctionCallError::RespondToModel(msg)) = result {
      assert!(msg.contains("最多读取"));
    }
  }

  #[tokio::test]
  async fn handler_rejects_empty_paths() {
    use serde_json::json;

    let dir = tempfile::tempdir().unwrap();
    let invocation = crate::tools::context::ToolInvocation {
      id: "test-3".to_string(),
      name: "read_many_files".to_string(),
      payload: crate::tools::context::ToolPayload::Function {
        arguments: json!({ "paths": [] }).to_string(),
      },
      cwd: dir.path().to_path_buf(),
      runtime: None,
    };

    let handler = ReadManyFilesHandler;
    let result = handler.handle_async(invocation).await;
    assert!(result.is_err());
  }

  #[tokio::test]
  async fn handler_continues_on_missing_file() {
    use serde_json::json;

    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().join("exists.txt");
    tokio::fs::write(&existing, "hello\n").await.unwrap();

    let invocation = crate::tools::context::ToolInvocation {
      id: "test-4".to_string(),
      name: "read_many_files".to_string(),
      payload: crate::tools::context::ToolPayload::Function {
        arguments: json!({
          "paths": [existing.to_string_lossy(), "/nonexistent/missing.txt"]
        })
        .to_string(),
      },
      cwd: dir.path().to_path_buf(),
      runtime: None,
    };

    let handler = ReadManyFilesHandler;
    // 不应该返回 Err — 单个文件失败不中断整批
    let output = handler.handle_async(invocation).await.unwrap();
    let text = output.text_content();
    assert!(text.contains("hello"));
    assert!(text.contains("Error:"));
  }
}
