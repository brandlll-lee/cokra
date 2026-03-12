//! save_memory tool handler — persist a fact to ~/.cokra/memory.md.
//!
//! Modelled after gemini-cli's MemoryTool:
//! - Appends under a `## Cokra Added Memories` section header.
//! - Sanitises the fact: collapses newlines, strips leading markdown bullets.
//! - Creates the memory file and parent dirs if they do not exist.

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct SaveMemoryHandler;

const MEMORY_SECTION_HEADER: &str = "## Cokra Added Memories";
const MEMORY_FILE_NAME: &str = "memory.md";

#[derive(Debug, Deserialize)]
struct SaveMemoryArgs {
  /// A clear, self-contained statement to remember.
  fact: String,
}

#[async_trait]
impl ToolHandler for SaveMemoryHandler {
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
    let args: SaveMemoryArgs = invocation.parse_arguments()?;

    let sanitized = sanitize_fact(&args.fact);
    if sanitized.is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "fact must not be empty".to_string(),
      ));
    }

    let memory_path = memory_file_path().map_err(|e| {
      FunctionCallError::Execution(format!("cannot determine memory file path: {e}"))
    })?;

    // Ensure parent directory exists
    if let Some(parent) = memory_path.parent() {
      tokio::fs::create_dir_all(parent).await.map_err(|e| {
        FunctionCallError::Execution(format!(
          "failed to create memory directory {}: {e}",
          parent.display()
        ))
      })?;
    }

    // Read existing content (empty string if file does not exist)
    let current_content = match tokio::fs::read_to_string(&memory_path).await {
      Ok(content) => content,
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
      Err(e) => {
        return Err(FunctionCallError::Execution(format!(
          "failed to read memory file: {e}"
        )));
      }
    };

    let new_content = append_memory(&current_content, &sanitized);

    tokio::fs::write(&memory_path, &new_content)
      .await
      .map_err(|e| FunctionCallError::Execution(format!("failed to write memory file: {e}")))?;

    let message = format!("Remembered: \"{sanitized}\"");
    Ok(ToolOutput::success(message).with_id(id))
  }
}

/// Returns `~/.cokra/memory.md`.
pub fn memory_file_path() -> Result<std::path::PathBuf, String> {
  dirs::home_dir()
    .ok_or_else(|| "cannot determine home directory".to_string())
    .map(|home| home.join(".cokra").join(MEMORY_FILE_NAME))
}

/// Sanitise the fact: collapse newlines to spaces, strip leading bullet chars.
fn sanitize_fact(fact: &str) -> String {
  let collapsed = fact.replace(['\r', '\n'], " ");
  let trimmed = collapsed.trim();
  // Strip leading markdown bullet (- or *) followed by optional whitespace
  let stripped = trimmed
    .trim_start_matches('-')
    .trim_start_matches('*')
    .trim_start();
  stripped.to_string()
}

/// Append a new memory entry to `current_content` under the section header.
/// Mirrors gemini-cli's `computeNewContent`.
fn append_memory(current_content: &str, fact: &str) -> String {
  let new_item = format!("- {fact}");

  match current_content.find(MEMORY_SECTION_HEADER) {
    None => {
      // No section yet — append header + entry
      let separator = newline_separator(current_content);
      format!("{current_content}{separator}{MEMORY_SECTION_HEADER}\n{new_item}\n")
    }
    Some(header_idx) => {
      let section_start = header_idx + MEMORY_SECTION_HEADER.len();
      // Find next ## section or end of file
      let section_end = current_content[section_start..]
        .find("\n## ")
        .map(|pos| section_start + pos)
        .unwrap_or(current_content.len());

      let before = current_content[..section_start].trim_end();
      let section_body = current_content[section_start..section_end].trim_end();
      let after = &current_content[section_end..];

      format!("{before}\n{section_body}\n{new_item}\n{after}")
        .trim_end()
        .to_string()
        + "\n"
    }
  }
}

/// Returns the appropriate newline separator to insert before a new block.
fn newline_separator(s: &str) -> &'static str {
  if s.is_empty() {
    ""
  } else if s.ends_with("\n\n") {
    ""
  } else if s.ends_with('\n') {
    "\n"
  } else {
    "\n\n"
  }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn sanitize_strips_leading_bullet() {
    assert_eq!(sanitize_fact("- hello world"), "hello world");
    assert_eq!(sanitize_fact("* hello world"), "hello world");
    assert_eq!(sanitize_fact("  - spaced"), "spaced");
  }

  #[test]
  fn sanitize_collapses_newlines() {
    assert_eq!(sanitize_fact("line one\nline two"), "line one line two");
  }

  #[test]
  fn sanitize_trims_whitespace() {
    assert_eq!(sanitize_fact("  fact  "), "fact");
  }

  #[test]
  fn sanitize_empty_returns_empty() {
    assert_eq!(sanitize_fact("  \n  "), "");
  }

  #[test]
  fn append_memory_empty_file() {
    let result = append_memory("", "the sky is blue");
    assert_eq!(
      result,
      format!("{MEMORY_SECTION_HEADER}\n- the sky is blue\n")
    );
  }

  #[test]
  fn append_memory_existing_file_no_section() {
    let existing = "# My Notes\n\nSome content.\n";
    let result = append_memory(existing, "rust is fast");
    assert!(result.contains("# My Notes"));
    assert!(result.contains(MEMORY_SECTION_HEADER));
    assert!(result.contains("- rust is fast"));
    // Section comes after existing content
    let header_pos = result.find(MEMORY_SECTION_HEADER).unwrap();
    let notes_pos = result.find("# My Notes").unwrap();
    assert!(header_pos > notes_pos);
  }

  #[test]
  fn append_memory_appends_to_existing_section() {
    let existing = format!("{MEMORY_SECTION_HEADER}\n- old fact\n");
    let result = append_memory(&existing, "new fact");
    assert!(result.contains("- old fact"));
    assert!(result.contains("- new fact"));
    // New fact comes after old fact
    let old_pos = result.find("- old fact").unwrap();
    let new_pos = result.find("- new fact").unwrap();
    assert!(new_pos > old_pos);
  }

  #[test]
  fn append_memory_preserves_after_section() {
    let existing = format!("{MEMORY_SECTION_HEADER}\n- old fact\n\n## Other Section\nContent\n");
    let result = append_memory(&existing, "another fact");
    assert!(result.contains("## Other Section"));
    assert!(result.contains("- another fact"));
  }

  #[test]
  fn newline_separator_empty() {
    assert_eq!(newline_separator(""), "");
  }

  #[test]
  fn newline_separator_ends_with_double_newline() {
    assert_eq!(newline_separator("content\n\n"), "");
  }

  #[test]
  fn newline_separator_ends_with_single_newline() {
    assert_eq!(newline_separator("content\n"), "\n");
  }

  #[test]
  fn newline_separator_no_trailing_newline() {
    assert_eq!(newline_separator("content"), "\n\n");
  }

  #[tokio::test]
  async fn rejects_empty_fact() {
    use crate::tools::context::ToolPayload;
    let inv = ToolInvocation {
      id: "t1".to_string(),
      name: "save_memory".to_string(),
      payload: ToolPayload::Function {
        arguments: r#"{"fact":"  \n  "}"#.to_string(),
      },
      cwd: std::env::temp_dir(),
      runtime: None,
    };
    let err = SaveMemoryHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("empty"));
  }
}
