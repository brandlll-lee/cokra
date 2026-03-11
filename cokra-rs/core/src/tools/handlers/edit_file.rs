//! edit_file tool handler — precise oldString/newString replacement.
//!
//! Modelled after OpenCode's `edit` tool. Supports:
//! - Exact match replacement (single or all occurrences)
//! - Create new file when old_string is empty
//! - CRLF normalisation
//! - Returns unified diff output + optional LSP diagnostics

use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

use super::diagnostics::collect_file_diagnostics;

pub struct EditFileHandler;

#[derive(Debug, Deserialize)]
struct EditFileArgs {
  file_path: String,
  old_string: String,
  new_string: String,
  #[serde(default)]
  replace_all: bool,
}

#[async_trait]
impl ToolHandler for EditFileHandler {
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
    let args: EditFileArgs = invocation.parse_arguments()?;

    let path = PathBuf::from(&args.file_path);
    if !path.is_absolute() {
      return Err(FunctionCallError::RespondToModel(
        "file_path must be an absolute path".to_string(),
      ));
    }

    if args.old_string == args.new_string {
      return Err(FunctionCallError::RespondToModel(
        "old_string and new_string must be different".to_string(),
      ));
    }

    // Create new file when old_string is empty
    if args.old_string.is_empty() {
      if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
      {
        fs::create_dir_all(parent).map_err(|e| {
          FunctionCallError::Execution(format!("failed to create {}: {e}", parent.display()))
        })?;
      }
      fs::write(&path, args.new_string.as_bytes()).map_err(|e| {
        FunctionCallError::Execution(format!("failed to write {}: {e}", path.display()))
      })?;
      let diag_suffix = collect_file_diagnostics(&path).await;
      return Ok(
        ToolOutput::success(format!(
          "Created new file: {}{}",
          path.display(),
          diag_suffix
        ))
        .with_id(id),
      );
    }

    // Read existing file
    let content = fs::read_to_string(&path).map_err(|e| {
      FunctionCallError::RespondToModel(format!("failed to read {}: {e}", path.display()))
    })?;

    // Normalise CRLF → LF for matching
    let normalised_content = content.replace("\r\n", "\n");
    let normalised_old = args.old_string.replace("\r\n", "\n");
    let normalised_new = args.new_string.replace("\r\n", "\n");

    // Count occurrences
    let count = normalised_content.matches(&normalised_old).count();
    if count == 0 {
      // Try line-trimmed fallback: strip leading/trailing whitespace per line
      let trimmed_content = trim_lines(&normalised_content);
      let trimmed_old = trim_lines(&normalised_old);
      if trimmed_content.contains(&trimmed_old) {
        return Err(FunctionCallError::RespondToModel(
          "old_string not found in file (exact match). A similar match exists with different \
           whitespace. Please provide the exact string including whitespace."
            .to_string(),
        ));
      }
      return Err(FunctionCallError::RespondToModel(
        "old_string not found in file. Please check the content and try again.".to_string(),
      ));
    }

    if count > 1 && !args.replace_all {
      return Err(FunctionCallError::RespondToModel(format!(
        "old_string found {count} times in file. Use replace_all: true to replace all \
         occurrences, or provide a more specific old_string that matches exactly once."
      )));
    }

    // Perform replacement
    let new_content = if args.replace_all {
      normalised_content.replace(&normalised_old, &normalised_new)
    } else {
      normalised_content.replacen(&normalised_old, &normalised_new, 1)
    };

    // Preserve original line endings: if file had CRLF, convert back
    let final_content = if content.contains("\r\n") {
      new_content.replace('\n', "\r\n")
    } else {
      new_content
    };

    fs::write(&path, final_content.as_bytes()).map_err(|e| {
      FunctionCallError::Execution(format!("failed to write {}: {e}", path.display()))
    })?;

    let replacements = if args.replace_all { count } else { 1 };
    let diff_summary = build_diff_summary(&normalised_old, &normalised_new, replacements);

    let diag_suffix = collect_file_diagnostics(&path).await;
    Ok(
      ToolOutput::success(format!(
        "Edit applied successfully to {}.\n{}{}",
        path.display(),
        diff_summary,
        diag_suffix
      ))
      .with_id(id),
    )
  }
}

/// Trim each line independently to enable fuzzy whitespace matching.
fn trim_lines(s: &str) -> String {
  s.lines().map(|l| l.trim()).collect::<Vec<_>>().join("\n")
}

/// Build a concise diff summary for the model.
fn build_diff_summary(old: &str, new: &str, replacements: usize) -> String {
  let old_lines = old.lines().count();
  let new_lines = new.lines().count();
  let delta = new_lines as isize - old_lines as isize;
  let delta_str = if delta > 0 {
    format!("+{delta} lines")
  } else if delta < 0 {
    format!("{delta} lines")
  } else {
    "same line count".to_string()
  };
  format!(
    "{replacements} replacement(s), {old_lines} lines removed, {new_lines} lines added ({delta_str})"
  )
}

#[cfg(test)]
mod tests {
  use std::fs;

  use super::EditFileHandler;
  use crate::tools::context::ToolInvocation;
  use crate::tools::context::ToolPayload;
  use crate::tools::registry::ToolHandler;

  fn make_inv(id: &str, args: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
      id: id.to_string(),
      name: "edit_file".to_string(),
      payload: ToolPayload::Function {
        arguments: args.to_string(),
      },
      cwd: std::env::temp_dir(),
      runtime: None,
    }
  }

  fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("cokra-edit-{}-{}.txt", name, uuid::Uuid::new_v4()))
  }

  #[tokio::test]
  async fn creates_new_file_when_old_string_empty() {
    let path = temp_path("create");
    let inv = make_inv(
      "1",
      serde_json::json!({
        "file_path": path.display().to_string(),
        "old_string": "",
        "new_string": "hello world"
      }),
    );
    let out = EditFileHandler.handle_async(inv).await.unwrap();
    assert!(!out.is_error());
    let content = fs::read_to_string(&path).unwrap();
    assert_eq!(content, "hello world");
    let _ = fs::remove_file(path);
  }

  #[tokio::test]
  async fn single_replacement() {
    let path = temp_path("single");
    fs::write(&path, "aaa bbb ccc").unwrap();
    let inv = make_inv(
      "2",
      serde_json::json!({
        "file_path": path.display().to_string(),
        "old_string": "bbb",
        "new_string": "BBB"
      }),
    );
    let out = EditFileHandler.handle_async(inv).await.unwrap();
    assert!(!out.is_error());
    assert_eq!(fs::read_to_string(&path).unwrap(), "aaa BBB ccc");
    let _ = fs::remove_file(path);
  }

  #[tokio::test]
  async fn replace_all() {
    let path = temp_path("replall");
    fs::write(&path, "foo bar foo baz foo").unwrap();
    let inv = make_inv(
      "3",
      serde_json::json!({
        "file_path": path.display().to_string(),
        "old_string": "foo",
        "new_string": "qux",
        "replace_all": true
      }),
    );
    let out = EditFileHandler.handle_async(inv).await.unwrap();
    assert!(!out.is_error());
    assert_eq!(fs::read_to_string(&path).unwrap(), "qux bar qux baz qux");
    let _ = fs::remove_file(path);
  }

  #[tokio::test]
  async fn rejects_multiple_matches_without_replace_all() {
    let path = temp_path("multi");
    fs::write(&path, "foo bar foo").unwrap();
    let inv = make_inv(
      "4",
      serde_json::json!({
        "file_path": path.display().to_string(),
        "old_string": "foo",
        "new_string": "baz"
      }),
    );
    let err = EditFileHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("2 times"));
    let _ = fs::remove_file(path);
  }

  #[tokio::test]
  async fn rejects_relative_path() {
    let inv = make_inv(
      "5",
      serde_json::json!({
        "file_path": "relative/file.txt",
        "old_string": "a",
        "new_string": "b"
      }),
    );
    let err = EditFileHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("absolute path"));
  }

  #[tokio::test]
  async fn rejects_same_old_new() {
    let inv = make_inv(
      "6",
      serde_json::json!({
        "file_path": "/tmp/x.txt",
        "old_string": "same",
        "new_string": "same"
      }),
    );
    let err = EditFileHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("must be different"));
  }

  #[tokio::test]
  async fn rejects_old_string_not_found() {
    let path = temp_path("notfound");
    fs::write(&path, "hello world").unwrap();
    let inv = make_inv(
      "7",
      serde_json::json!({
        "file_path": path.display().to_string(),
        "old_string": "missing",
        "new_string": "replacement"
      }),
    );
    let err = EditFileHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("not found"));
    let _ = fs::remove_file(path);
  }

  #[tokio::test]
  async fn preserves_crlf_line_endings() {
    let path = temp_path("crlf");
    fs::write(&path, "line1\r\nline2\r\nline3").unwrap();
    let inv = make_inv(
      "8",
      serde_json::json!({
        "file_path": path.display().to_string(),
        "old_string": "line2",
        "new_string": "LINE2"
      }),
    );
    let out = EditFileHandler.handle_async(inv).await.unwrap();
    assert!(!out.is_error());
    assert_eq!(
      fs::read_to_string(&path).unwrap(),
      "line1\r\nLINE2\r\nline3"
    );
    let _ = fs::remove_file(path);
  }

  #[tokio::test]
  async fn multiline_replacement() {
    let path = temp_path("multiline");
    fs::write(&path, "fn main() {\n  println!(\"old\");\n}\n").unwrap();
    let inv = make_inv(
      "9",
      serde_json::json!({
        "file_path": path.display().to_string(),
        "old_string": "  println!(\"old\");",
        "new_string": "  println!(\"new\");\n  println!(\"extra\");"
      }),
    );
    let out = EditFileHandler.handle_async(inv).await.unwrap();
    assert!(!out.is_error());
    assert_eq!(
      fs::read_to_string(&path).unwrap(),
      "fn main() {\n  println!(\"new\");\n  println!(\"extra\");\n}\n"
    );
    let _ = fs::remove_file(path);
  }

  #[tokio::test]
  async fn whitespace_hint_when_trimmed_match_exists() {
    let path = temp_path("wshint");
    fs::write(&path, "  hello  \n  world  \n").unwrap();
    let inv = make_inv(
      "10",
      serde_json::json!({
        "file_path": path.display().to_string(),
        "old_string": "hello\nworld",
        "new_string": "replaced"
      }),
    );
    let err = EditFileHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("whitespace"));
    let _ = fs::remove_file(path);
  }
}
