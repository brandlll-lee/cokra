//! 1:1 codex: read_file tool handler — requires absolute paths.
//!
//! Unlike grep_files/shell which resolve relative paths against session cwd,
//! read_file rejects relative paths outright. The model is expected to send
//! absolute paths (it learns the cwd from the environment_context message).

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::fs::File;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ReadFileHandler;

const MAX_LINE_LENGTH: usize = 500;

fn default_offset() -> usize {
  1
}

fn default_limit() -> usize {
  2000
}

#[derive(Deserialize)]
struct ReadFileArgs {
  file_path: String,
  #[serde(default = "default_offset")]
  offset: usize,
  #[serde(default = "default_limit")]
  limit: usize,
}

#[async_trait]
impl ToolHandler for ReadFileHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let id = invocation.id.clone();
    let args: ReadFileArgs = invocation.parse_arguments()?;

    let ReadFileArgs {
      file_path,
      offset,
      limit,
    } = args;

    if offset == 0 {
      return Err(FunctionCallError::RespondToModel(
        "offset must be a 1-indexed line number".to_string(),
      ));
    }

    if limit == 0 {
      return Err(FunctionCallError::RespondToModel(
        "limit must be greater than zero".to_string(),
      ));
    }

    let path = PathBuf::from(&file_path);
    if !path.is_absolute() {
      return Err(FunctionCallError::RespondToModel(
        "file_path must be an absolute path".to_string(),
      ));
    }

    let collected = read_slice(&path, offset, limit).await?;
    let mut out = ToolOutput::success(collected.join("\n"));
    out.id = id;
    Ok(out)
  }
}

/// Truncate a string at a char boundary, returning at most `max_bytes` bytes.
fn take_at_char_boundary(s: &str, max_bytes: usize) -> &str {
  if s.len() <= max_bytes {
    return s;
  }
  let mut end = max_bytes;
  while end > 0 && !s.is_char_boundary(end) {
    end -= 1;
  }
  &s[..end]
}

fn format_line(bytes: &[u8]) -> String {
  let decoded = String::from_utf8_lossy(bytes);
  if decoded.len() > MAX_LINE_LENGTH {
    take_at_char_boundary(&decoded, MAX_LINE_LENGTH).to_string()
  } else {
    decoded.into_owned()
  }
}

async fn read_slice(
  path: &std::path::Path,
  offset: usize,
  limit: usize,
) -> Result<Vec<String>, FunctionCallError> {
  let file = File::open(path)
    .await
    .map_err(|err| FunctionCallError::RespondToModel(format!("failed to read file: {err}")))?;

  let mut reader = BufReader::new(file);
  let mut collected = Vec::new();
  let mut seen = 0usize;
  let mut buffer = Vec::new();

  loop {
    buffer.clear();
    let bytes_read = reader
      .read_until(b'\n', &mut buffer)
      .await
      .map_err(|err| FunctionCallError::RespondToModel(format!("failed to read file: {err}")))?;

    if bytes_read == 0 {
      break;
    }

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

    if collected.len() == limit {
      break;
    }

    let formatted = format_line(&buffer);
    collected.push(format!("L{seen}: {formatted}"));

    if collected.len() == limit {
      break;
    }
  }

  if seen < offset {
    return Err(FunctionCallError::RespondToModel(
      "offset exceeds file length".to_string(),
    ));
  }

  Ok(collected)
}

#[cfg(test)]
mod tests {
  use super::*;
  use pretty_assertions::assert_eq;
  use tempfile::NamedTempFile;

  #[tokio::test]
  async fn reads_requested_range() {
    let mut temp = NamedTempFile::new().expect("create temp");
    use std::io::Write as _;
    write!(temp, "alpha\nbeta\ngamma\n").expect("write");

    let lines = read_slice(temp.path(), 2, 2).await.expect("read");
    assert_eq!(lines, vec!["L2: beta".to_string(), "L3: gamma".to_string()]);
  }

  #[tokio::test]
  async fn errors_when_offset_exceeds_length() {
    let mut temp = NamedTempFile::new().expect("create temp");
    use std::io::Write as _;
    writeln!(temp, "only").expect("write");

    let err = read_slice(temp.path(), 3, 1)
      .await
      .expect_err("offset exceeds length");
    assert!(
      matches!(err, FunctionCallError::RespondToModel(ref msg) if msg.contains("offset exceeds"))
    );
  }

  #[tokio::test]
  async fn reads_non_utf8_lines() {
    let mut temp = NamedTempFile::new().expect("create temp");
    use std::io::Write as _;
    temp
      .as_file_mut()
      .write_all(b"\xff\xfe\nplain\n")
      .expect("write");

    let lines = read_slice(temp.path(), 1, 2).await.expect("read");
    let expected_first = format!("L1: {}{}", '\u{FFFD}', '\u{FFFD}');
    assert_eq!(lines, vec![expected_first, "L2: plain".to_string()]);
  }

  #[tokio::test]
  async fn trims_crlf_endings() {
    let mut temp = NamedTempFile::new().expect("create temp");
    use std::io::Write as _;
    write!(temp, "one\r\ntwo\r\n").expect("write");

    let lines = read_slice(temp.path(), 1, 2).await.expect("read");
    assert_eq!(lines, vec!["L1: one".to_string(), "L2: two".to_string()]);
  }

  #[tokio::test]
  async fn respects_limit_even_with_more_lines() {
    let mut temp = NamedTempFile::new().expect("create temp");
    use std::io::Write as _;
    write!(temp, "first\nsecond\nthird\n").expect("write");

    let lines = read_slice(temp.path(), 1, 2).await.expect("read");
    assert_eq!(
      lines,
      vec!["L1: first".to_string(), "L2: second".to_string()]
    );
  }

  #[tokio::test]
  async fn truncates_lines_longer_than_max_length() {
    let mut temp = NamedTempFile::new().expect("create temp");
    use std::io::Write as _;
    let long_line = "x".repeat(MAX_LINE_LENGTH + 50);
    writeln!(temp, "{long_line}").expect("write");

    let lines = read_slice(temp.path(), 1, 1).await.expect("read");
    let expected = "x".repeat(MAX_LINE_LENGTH);
    assert_eq!(lines, vec![format!("L1: {expected}")]);
  }

  #[tokio::test]
  async fn rejects_relative_path() {
    let invocation = ToolInvocation {
      id: "2".to_string(),
      name: "read_file".to_string(),
      arguments: serde_json::json!({
        "file_path": "relative/path.rs"
      })
      .to_string(),
      cwd: std::env::temp_dir(),
      runtime: None,
    };

    let err = ReadFileHandler
      .handle_async(invocation)
      .await
      .expect_err("should reject relative path");
    assert!(err.to_string().contains("absolute path"));
  }

  #[tokio::test]
  async fn default_offset_is_one_indexed() {
    let mut temp = NamedTempFile::new().expect("create temp");
    use std::io::Write as _;
    write!(temp, "first\nsecond\n").expect("write");

    // Omitting offset should default to 1 (first line)
    let lines = read_slice(temp.path(), 1, 1).await.expect("read");
    assert_eq!(lines, vec!["L1: first".to_string()]);
  }

  #[tokio::test]
  async fn directory_path_returns_error() {
    let temp = tempfile::tempdir().expect("create tempdir");

    let err = read_slice(temp.path(), 1, 1)
      .await
      .expect_err("should fail on directory");
    assert!(err.to_string().contains("failed to read file"));
  }
}
