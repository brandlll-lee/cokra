//! 1:1 codex: apply-patch crate — parse and apply patch diffs to the filesystem.

mod parser;
mod seek_sequence;

use std::path::Path;
use std::path::PathBuf;

pub use parser::Hunk;
pub use parser::ParseError;
pub use parser::ParsedPatch;
pub use parser::UpdateFileChunk;
pub use parser::parse_patch;
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum ApplyPatchError {
  #[error(transparent)]
  ParseError(#[from] ParseError),
  #[error(transparent)]
  IoError(#[from] IoError),
  #[error("{0}")]
  ComputeReplacements(String),
}

impl From<std::io::Error> for ApplyPatchError {
  fn from(err: std::io::Error) -> Self {
    ApplyPatchError::IoError(IoError {
      context: "I/O error".to_string(),
      source: err,
    })
  }
}

#[derive(Debug, Error)]
#[error("{context}: {source}")]
pub struct IoError {
  context: String,
  #[source]
  source: std::io::Error,
}

impl PartialEq for IoError {
  fn eq(&self, other: &Self) -> bool {
    self.context == other.context && self.source.to_string() == other.source.to_string()
  }
}

/// Tracks file paths affected by applying a patch.
pub struct AffectedPaths {
  pub added: Vec<PathBuf>,
  pub modified: Vec<PathBuf>,
  pub deleted: Vec<PathBuf>,
}

/// Apply a patch string to the filesystem. `cwd` is used to resolve relative
/// paths in the patch.
pub fn apply_patch(patch: &str, cwd: &Path) -> Result<AffectedPaths, ApplyPatchError> {
  let parsed = parse_patch(patch)?;
  if parsed.hunks.is_empty() {
    return Err(ApplyPatchError::ComputeReplacements(
      "No files were modified.".to_string(),
    ));
  }

  // Resolve all paths relative to cwd.
  let resolved_hunks: Vec<Hunk> = parsed
    .hunks
    .into_iter()
    .map(|hunk| resolve_hunk(hunk, cwd))
    .collect();

  apply_hunks_to_files(&resolved_hunks)
}

/// Resolve relative paths in a hunk to absolute paths using `cwd`.
fn resolve_hunk(hunk: Hunk, cwd: &Path) -> Hunk {
  match hunk {
    Hunk::AddFile { path, contents } => {
      let resolved = if path.is_absolute() {
        path
      } else {
        cwd.join(path)
      };
      Hunk::AddFile {
        path: resolved,
        contents,
      }
    }
    Hunk::DeleteFile { path } => {
      let resolved = if path.is_absolute() {
        path
      } else {
        cwd.join(path)
      };
      Hunk::DeleteFile { path: resolved }
    }
    Hunk::UpdateFile {
      path,
      move_path,
      chunks,
    } => {
      let resolved = if path.is_absolute() {
        path
      } else {
        cwd.join(path)
      };
      let resolved_move = move_path.map(|mp| if mp.is_absolute() { mp } else { cwd.join(mp) });
      Hunk::UpdateFile {
        path: resolved,
        move_path: resolved_move,
        chunks,
      }
    }
  }
}

/// Apply the hunks to the filesystem.
fn apply_hunks_to_files(hunks: &[Hunk]) -> Result<AffectedPaths, ApplyPatchError> {
  let mut added: Vec<PathBuf> = Vec::new();
  let mut modified: Vec<PathBuf> = Vec::new();
  let mut deleted: Vec<PathBuf> = Vec::new();

  for hunk in hunks {
    match hunk {
      Hunk::AddFile { path, contents } => {
        if let Some(parent) = path.parent() {
          if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
              ApplyPatchError::IoError(IoError {
                context: format!("Failed to create parent directories for {}", path.display()),
                source: e,
              })
            })?;
          }
        }
        std::fs::write(path, contents).map_err(|e| {
          ApplyPatchError::IoError(IoError {
            context: format!("Failed to write file {}", path.display()),
            source: e,
          })
        })?;
        added.push(path.clone());
      }
      Hunk::DeleteFile { path } => {
        std::fs::remove_file(path).map_err(|e| {
          ApplyPatchError::IoError(IoError {
            context: format!("Failed to delete file {}", path.display()),
            source: e,
          })
        })?;
        deleted.push(path.clone());
      }
      Hunk::UpdateFile {
        path,
        move_path,
        chunks,
      } => {
        let new_contents = derive_new_contents_from_chunks(path, chunks)?;
        if let Some(dest) = move_path {
          if let Some(parent) = dest.parent() {
            if !parent.as_os_str().is_empty() {
              std::fs::create_dir_all(parent).map_err(|e| {
                ApplyPatchError::IoError(IoError {
                  context: format!("Failed to create parent directories for {}", dest.display()),
                  source: e,
                })
              })?;
            }
          }
          std::fs::write(dest, &new_contents).map_err(|e| {
            ApplyPatchError::IoError(IoError {
              context: format!("Failed to write file {}", dest.display()),
              source: e,
            })
          })?;
          std::fs::remove_file(path).map_err(|e| {
            ApplyPatchError::IoError(IoError {
              context: format!("Failed to remove original {}", path.display()),
              source: e,
            })
          })?;
          modified.push(dest.clone());
        } else {
          std::fs::write(path, &new_contents).map_err(|e| {
            ApplyPatchError::IoError(IoError {
              context: format!("Failed to write file {}", path.display()),
              source: e,
            })
          })?;
          modified.push(path.clone());
        }
      }
    }
  }

  Ok(AffectedPaths {
    added,
    modified,
    deleted,
  })
}

/// Derive the new file contents after applying update chunks to an existing
/// file.
fn derive_new_contents_from_chunks(
  path: &Path,
  chunks: &[UpdateFileChunk],
) -> Result<String, ApplyPatchError> {
  let original_contents = std::fs::read_to_string(path).map_err(|e| {
    ApplyPatchError::IoError(IoError {
      context: format!("Failed to read file to update {}", path.display()),
      source: e,
    })
  })?;

  let mut original_lines: Vec<String> = original_contents.split('\n').map(String::from).collect();

  // Drop the trailing empty element that results from the final newline so
  // that line counts match the behaviour of standard `diff`.
  if original_lines.last().is_some_and(String::is_empty) {
    original_lines.pop();
  }

  let replacements = compute_replacements(&original_lines, path, chunks)?;
  let mut new_lines = apply_replacements(original_lines, &replacements);
  if !new_lines.last().is_some_and(String::is_empty) {
    new_lines.push(String::new());
  }
  Ok(new_lines.join("\n"))
}

/// Compute a list of replacements needed to transform `original_lines` into the
/// new lines. Each replacement is `(start_index, old_len, new_lines)`.
fn compute_replacements(
  original_lines: &[String],
  path: &Path,
  chunks: &[UpdateFileChunk],
) -> Result<Vec<(usize, usize, Vec<String>)>, ApplyPatchError> {
  let mut replacements: Vec<(usize, usize, Vec<String>)> = Vec::new();
  let mut line_index: usize = 0;

  for chunk in chunks {
    if let Some(ctx_line) = &chunk.change_context {
      if let Some(idx) = seek_sequence::seek_sequence(
        original_lines,
        std::slice::from_ref(ctx_line),
        line_index,
        false,
      ) {
        line_index = idx + 1;
      } else {
        return Err(ApplyPatchError::ComputeReplacements(format!(
          "Failed to find context '{}' in {}",
          ctx_line,
          path.display()
        )));
      }
    }

    if chunk.old_lines.is_empty() {
      let insertion_idx = if original_lines.last().is_some_and(String::is_empty) {
        original_lines.len() - 1
      } else {
        original_lines.len()
      };
      replacements.push((insertion_idx, 0, chunk.new_lines.clone()));
      continue;
    }

    let mut pattern: &[String] = &chunk.old_lines;
    let mut found =
      seek_sequence::seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);

    let mut new_slice: &[String] = &chunk.new_lines;

    if found.is_none() && pattern.last().is_some_and(String::is_empty) {
      pattern = &pattern[..pattern.len() - 1];
      if new_slice.last().is_some_and(String::is_empty) {
        new_slice = &new_slice[..new_slice.len() - 1];
      }

      found =
        seek_sequence::seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
    }

    if let Some(start_idx) = found {
      replacements.push((start_idx, pattern.len(), new_slice.to_vec()));
      line_index = start_idx + pattern.len();
    } else {
      return Err(ApplyPatchError::ComputeReplacements(format!(
        "Failed to find expected lines in {}:\n{}",
        path.display(),
        chunk.old_lines.join("\n"),
      )));
    }
  }

  replacements.sort_by(|(lhs_idx, _, _), (rhs_idx, _, _)| lhs_idx.cmp(rhs_idx));

  Ok(replacements)
}

/// Apply replacements in descending order so earlier replacements don't shift
/// positions of later ones.
fn apply_replacements(
  mut lines: Vec<String>,
  replacements: &[(usize, usize, Vec<String>)],
) -> Vec<String> {
  for (start_idx, old_len, new_segment) in replacements.iter().rev() {
    let start_idx = *start_idx;
    let old_len = *old_len;

    for _ in 0..old_len {
      if start_idx < lines.len() {
        lines.remove(start_idx);
      }
    }

    for (offset, new_line) in new_segment.iter().enumerate() {
      lines.insert(start_idx + offset, new_line.clone());
    }
  }

  lines
}

/// Format a summary of affected paths.
pub fn format_summary(affected: &AffectedPaths) -> String {
  let mut out = String::new();
  for path in &affected.added {
    out.push_str(&format!("A {}\n", path.display()));
  }
  for path in &affected.modified {
    out.push_str(&format!("M {}\n", path.display()));
  }
  for path in &affected.deleted {
    out.push_str(&format!("D {}\n", path.display()));
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::fs;
  use tempfile::tempdir;

  fn wrap_patch(body: &str) -> String {
    format!("*** Begin Patch\n{body}\n*** End Patch")
  }

  #[test]
  fn test_add_file_creates_file() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("add.txt");
    let patch = wrap_patch(&format!("*** Add File: {}\n+ab\n+cd", path.display()));
    let affected = apply_patch(&patch, dir.path()).expect("apply");
    assert_eq!(affected.added.len(), 1);
    let contents = fs::read_to_string(&path).expect("read");
    assert_eq!(contents, "ab\ncd\n");
  }

  #[test]
  fn test_delete_file_removes_file() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("del.txt");
    fs::write(&path, "x").expect("write");
    let patch = wrap_patch(&format!("*** Delete File: {}", path.display()));
    let affected = apply_patch(&patch, dir.path()).expect("apply");
    assert_eq!(affected.deleted.len(), 1);
    assert!(!path.exists());
  }

  #[test]
  fn test_update_file_modifies_content() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("update.txt");
    fs::write(&path, "foo\nbar\n").expect("write");
    let patch = wrap_patch(&format!(
      "*** Update File: {}\n@@\n foo\n-bar\n+baz",
      path.display()
    ));
    let affected = apply_patch(&patch, dir.path()).expect("apply");
    assert_eq!(affected.modified.len(), 1);
    let contents = fs::read_to_string(&path).expect("read");
    assert_eq!(contents, "foo\nbaz\n");
  }

  #[test]
  fn test_update_file_move() {
    let dir = tempdir().expect("tempdir");
    let src = dir.path().join("src.txt");
    let dest = dir.path().join("dst.txt");
    fs::write(&src, "line\n").expect("write");
    let patch = wrap_patch(&format!(
      "*** Update File: {}\n*** Move to: {}\n@@\n-line\n+line2",
      src.display(),
      dest.display()
    ));
    let affected = apply_patch(&patch, dir.path()).expect("apply");
    assert_eq!(affected.modified.len(), 1);
    assert!(!src.exists());
    let contents = fs::read_to_string(&dest).expect("read");
    assert_eq!(contents, "line2\n");
  }

  #[test]
  fn test_multiple_update_chunks() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("multi.txt");
    fs::write(&path, "foo\nbar\nbaz\nqux\n").expect("write");
    let patch = wrap_patch(&format!(
      "*** Update File: {}\n@@\n foo\n-bar\n+BAR\n@@\n baz\n-qux\n+QUX",
      path.display()
    ));
    let affected = apply_patch(&patch, dir.path()).expect("apply");
    assert_eq!(affected.modified.len(), 1);
    let contents = fs::read_to_string(&path).expect("read");
    assert_eq!(contents, "foo\nBAR\nbaz\nQUX\n");
  }

  #[test]
  fn test_interleaved_changes() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("interleaved.txt");
    fs::write(&path, "a\nb\nc\nd\ne\nf\n").expect("write");
    let patch = wrap_patch(&format!(
      "*** Update File: {}\n\
             @@\n a\n-b\n+B\n\
             @@\n c\n d\n-e\n+E\n\
             @@\n f\n+g\n*** End of File",
      path.display()
    ));
    let affected = apply_patch(&patch, dir.path()).expect("apply");
    assert_eq!(affected.modified.len(), 1);
    let contents = fs::read_to_string(&path).expect("read");
    assert_eq!(contents, "a\nB\nc\nd\nE\nf\ng\n");
  }

  #[test]
  fn test_relative_path_resolution() {
    let dir = tempdir().expect("tempdir");
    let patch = wrap_patch("*** Add File: subdir/new.txt\n+hello");
    let affected = apply_patch(&patch, dir.path()).expect("apply");
    assert_eq!(affected.added.len(), 1);
    let contents = fs::read_to_string(dir.path().join("subdir/new.txt")).expect("read");
    assert_eq!(contents, "hello\n");
  }

  #[test]
  fn test_empty_patch_is_error() {
    let dir = tempdir().expect("tempdir");
    let patch = "*** Begin Patch\n*** End Patch";
    let result = apply_patch(patch, dir.path());
    assert!(result.is_err());
  }

  #[test]
  fn test_format_summary() {
    let affected = AffectedPaths {
      added: vec![PathBuf::from("a.txt")],
      modified: vec![PathBuf::from("m.txt")],
      deleted: vec![PathBuf::from("d.txt")],
    };
    let summary = format_summary(&affected);
    assert!(summary.contains("A a.txt"));
    assert!(summary.contains("M m.txt"));
    assert!(summary.contains("D d.txt"));
  }

  #[test]
  fn test_update_with_context_line() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("ctx.py");
    fs::write(&path, "def f():\n    pass\n    return\n").expect("write");
    let patch = wrap_patch(&format!(
      "*** Update File: {}\n@@ def f():\n-    pass\n+    x = 1",
      path.display()
    ));
    let affected = apply_patch(&patch, dir.path()).expect("apply");
    assert_eq!(affected.modified.len(), 1);
    let contents = fs::read_to_string(&path).expect("read");
    assert_eq!(contents, "def f():\n    x = 1\n    return\n");
  }
}
