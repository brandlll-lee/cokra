//! 1:1 codex: Parse & validate a patch into a list of "hunks".
//!
//! The official Lark grammar for the apply-patch format is:
//!
//! start: begin_patch hunk+ end_patch
//! begin_patch: "*** Begin Patch" LF
//! end_patch: "*** End Patch" LF?
//!
//! hunk: add_hunk | delete_hunk | update_hunk
//! add_hunk: "*** Add File: " filename LF add_line+
//! delete_hunk: "*** Delete File: " filename LF
//! update_hunk: "*** Update File: " filename LF change_move? change?
//! filename: /(.+)/
//! add_line: "+" /(.+)/ LF -> line
//!
//! change_move: "*** Move to: " filename LF
//! change: (change_context | change_line)+ eof_line?
//! change_context: ("@@" | "@@ " /(.+)/) LF
//! change_line: ("+" | "-" | " ") /(.+)/ LF
//! eof_line: "*** End of File" LF

use std::path::PathBuf;

use thiserror::Error;

const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
const END_PATCH_MARKER: &str = "*** End Patch";
const ADD_FILE_MARKER: &str = "*** Add File: ";
const DELETE_FILE_MARKER: &str = "*** Delete File: ";
const UPDATE_FILE_MARKER: &str = "*** Update File: ";
const MOVE_TO_MARKER: &str = "*** Move to: ";
const EOF_MARKER: &str = "*** End of File";
const CHANGE_CONTEXT_MARKER: &str = "@@ ";
const EMPTY_CHANGE_CONTEXT_MARKER: &str = "@@";

/// 1:1 codex: lenient parsing enabled for all models.
const PARSE_IN_STRICT_MODE: bool = false;

#[derive(Debug, PartialEq, Error, Clone)]
pub enum ParseError {
  #[error("invalid patch: {0}")]
  InvalidPatchError(String),
  #[error("invalid hunk at line {line_number}, {message}")]
  InvalidHunkError { message: String, line_number: usize },
}
use ParseError::*;

#[derive(Debug, PartialEq, Clone)]
pub enum Hunk {
  AddFile {
    path: PathBuf,
    contents: String,
  },
  DeleteFile {
    path: PathBuf,
  },
  UpdateFile {
    path: PathBuf,
    move_path: Option<PathBuf>,
    chunks: Vec<UpdateFileChunk>,
  },
}

use std::path::Path;

impl Hunk {
  pub fn resolve_path(&self, cwd: &Path) -> PathBuf {
    match self {
      Hunk::AddFile { path, .. } => cwd.join(path),
      Hunk::DeleteFile { path } => cwd.join(path),
      Hunk::UpdateFile { path, .. } => cwd.join(path),
    }
  }
}

use Hunk::*;

#[derive(Debug, PartialEq, Clone)]
pub struct UpdateFileChunk {
  pub change_context: Option<String>,
  pub old_lines: Vec<String>,
  pub new_lines: Vec<String>,
  pub is_end_of_file: bool,
}

/// Parsed patch result.
#[derive(Debug, PartialEq)]
pub struct ParsedPatch {
  pub hunks: Vec<Hunk>,
  pub patch: String,
}

pub fn parse_patch(patch: &str) -> Result<ParsedPatch, ParseError> {
  let mode = if PARSE_IN_STRICT_MODE {
    ParseMode::Strict
  } else {
    ParseMode::Lenient
  };
  parse_patch_text(patch, mode)
}

enum ParseMode {
  Strict,
  Lenient,
}

fn parse_patch_text(patch: &str, mode: ParseMode) -> Result<ParsedPatch, ParseError> {
  let lines: Vec<&str> = patch.trim().lines().collect();
  let lines: &[&str] = match check_patch_boundaries_strict(&lines) {
    Ok(()) => &lines,
    Err(e) => match mode {
      ParseMode::Strict => {
        return Err(e);
      }
      ParseMode::Lenient => check_patch_boundaries_lenient(&lines, e)?,
    },
  };

  let mut hunks: Vec<Hunk> = Vec::new();
  let last_line_index = lines.len().saturating_sub(1);
  let mut remaining_lines = &lines[1..last_line_index];
  let mut line_number = 2;
  while !remaining_lines.is_empty() {
    let (hunk, hunk_lines) = parse_one_hunk(remaining_lines, line_number)?;
    hunks.push(hunk);
    line_number += hunk_lines;
    remaining_lines = &remaining_lines[hunk_lines..]
  }
  let patch = lines.join("\n");
  Ok(ParsedPatch { hunks, patch })
}

fn check_patch_boundaries_strict(lines: &[&str]) -> Result<(), ParseError> {
  let (first_line, last_line) = match lines {
    [] => (None, None),
    [first] => (Some(first), Some(first)),
    [first, .., last] => (Some(first), Some(last)),
  };
  check_start_and_end_lines_strict(first_line, last_line)
}

fn check_patch_boundaries_lenient<'a>(
  original_lines: &'a [&'a str],
  original_parse_error: ParseError,
) -> Result<&'a [&'a str], ParseError> {
  match original_lines {
    [first, .., last] => {
      if (first == &"<<EOF" || first == &"<<'EOF'" || first == &"<<\"EOF\"")
        && last.ends_with("EOF")
        && original_lines.len() >= 4
      {
        let inner_lines = &original_lines[1..original_lines.len() - 1];
        match check_patch_boundaries_strict(inner_lines) {
          Ok(()) => Ok(inner_lines),
          Err(e) => Err(e),
        }
      } else {
        Err(original_parse_error)
      }
    }
    _ => Err(original_parse_error),
  }
}

fn check_start_and_end_lines_strict(
  first_line: Option<&&str>,
  last_line: Option<&&str>,
) -> Result<(), ParseError> {
  let first_line = first_line.map(|line| line.trim());
  let last_line = last_line.map(|line| line.trim());

  match (first_line, last_line) {
    (Some(first), Some(last)) if first == BEGIN_PATCH_MARKER && last == END_PATCH_MARKER => Ok(()),
    (Some(first), _) if first != BEGIN_PATCH_MARKER => Err(InvalidPatchError(String::from(
      "The first line of the patch must be '*** Begin Patch'",
    ))),
    _ => Err(InvalidPatchError(String::from(
      "The last line of the patch must be '*** End Patch'",
    ))),
  }
}

fn parse_one_hunk(lines: &[&str], line_number: usize) -> Result<(Hunk, usize), ParseError> {
  let first_line = lines[0].trim();
  if let Some(path) = first_line.strip_prefix(ADD_FILE_MARKER) {
    let mut contents = String::new();
    let mut parsed_lines = 1;
    for add_line in &lines[1..] {
      if let Some(line_to_add) = add_line.strip_prefix('+') {
        contents.push_str(line_to_add);
        contents.push('\n');
        parsed_lines += 1;
      } else {
        break;
      }
    }
    return Ok((
      AddFile {
        path: PathBuf::from(path),
        contents,
      },
      parsed_lines,
    ));
  } else if let Some(path) = first_line.strip_prefix(DELETE_FILE_MARKER) {
    return Ok((
      DeleteFile {
        path: PathBuf::from(path),
      },
      1,
    ));
  } else if let Some(path) = first_line.strip_prefix(UPDATE_FILE_MARKER) {
    let mut remaining_lines = &lines[1..];
    let mut parsed_lines = 1;

    let move_path = remaining_lines
      .first()
      .and_then(|x| x.strip_prefix(MOVE_TO_MARKER));

    if move_path.is_some() {
      remaining_lines = &remaining_lines[1..];
      parsed_lines += 1;
    }

    let mut chunks = Vec::new();
    while !remaining_lines.is_empty() {
      if remaining_lines[0].trim().is_empty() {
        parsed_lines += 1;
        remaining_lines = &remaining_lines[1..];
        continue;
      }

      if remaining_lines[0].starts_with("***") {
        break;
      }

      let (chunk, chunk_lines) = parse_update_file_chunk(
        remaining_lines,
        line_number + parsed_lines,
        chunks.is_empty(),
      )?;
      chunks.push(chunk);
      parsed_lines += chunk_lines;
      remaining_lines = &remaining_lines[chunk_lines..]
    }

    if chunks.is_empty() {
      return Err(InvalidHunkError {
        message: format!("Update file hunk for path '{path}' is empty"),
        line_number,
      });
    }

    return Ok((
      UpdateFile {
        path: PathBuf::from(path),
        move_path: move_path.map(PathBuf::from),
        chunks,
      },
      parsed_lines,
    ));
  }

  Err(InvalidHunkError {
    message: format!(
      "'{first_line}' is not a valid hunk header. Valid hunk headers: '*** Add File: {{path}}', '*** Delete File: {{path}}', '*** Update File: {{path}}'"
    ),
    line_number,
  })
}

fn parse_update_file_chunk(
  lines: &[&str],
  line_number: usize,
  allow_missing_context: bool,
) -> Result<(UpdateFileChunk, usize), ParseError> {
  if lines.is_empty() {
    return Err(InvalidHunkError {
      message: "Update hunk does not contain any lines".to_string(),
      line_number,
    });
  }
  let (change_context, start_index) = if lines[0] == EMPTY_CHANGE_CONTEXT_MARKER {
    (None, 1)
  } else if let Some(context) = lines[0].strip_prefix(CHANGE_CONTEXT_MARKER) {
    (Some(context.to_string()), 1)
  } else {
    if !allow_missing_context {
      return Err(InvalidHunkError {
        message: format!(
          "Expected update hunk to start with a @@ context marker, got: '{}'",
          lines[0]
        ),
        line_number,
      });
    }
    (None, 0)
  };
  if start_index >= lines.len() {
    return Err(InvalidHunkError {
      message: "Update hunk does not contain any lines".to_string(),
      line_number: line_number + 1,
    });
  }
  let mut chunk = UpdateFileChunk {
    change_context,
    old_lines: Vec::new(),
    new_lines: Vec::new(),
    is_end_of_file: false,
  };
  let mut parsed_lines = 0;
  for line in &lines[start_index..] {
    match *line {
      EOF_MARKER => {
        if parsed_lines == 0 {
          return Err(InvalidHunkError {
            message: "Update hunk does not contain any lines".to_string(),
            line_number: line_number + 1,
          });
        }
        chunk.is_end_of_file = true;
        parsed_lines += 1;
        break;
      }
      line_contents => {
        match line_contents.chars().next() {
          None => {
            chunk.old_lines.push(String::new());
            chunk.new_lines.push(String::new());
          }
          Some(' ') => {
            chunk.old_lines.push(line_contents[1..].to_string());
            chunk.new_lines.push(line_contents[1..].to_string());
          }
          Some('+') => {
            chunk.new_lines.push(line_contents[1..].to_string());
          }
          Some('-') => {
            chunk.old_lines.push(line_contents[1..].to_string());
          }
          _ => {
            if parsed_lines == 0 {
              return Err(InvalidHunkError {
                message: format!(
                  "Unexpected line found in update hunk: '{line_contents}'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)"
                ),
                line_number: line_number + 1,
              });
            }
            break;
          }
        }
        parsed_lines += 1;
      }
    }
  }

  Ok((chunk, parsed_lines + start_index))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_bad_patch_first_line() {
    assert!(matches!(parse_patch("bad"), Err(InvalidPatchError(_))));
  }

  #[test]
  fn test_bad_patch_last_line() {
    assert!(matches!(
      parse_patch("*** Begin Patch\nbad"),
      Err(InvalidPatchError(_))
    ));
  }

  #[test]
  fn test_add_file_hunk() {
    let result = parse_patch("*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch");
    let parsed = result.expect("should parse");
    assert_eq!(parsed.hunks.len(), 1);
    match &parsed.hunks[0] {
      AddFile { path, contents } => {
        assert_eq!(path, &PathBuf::from("foo"));
        assert_eq!(contents, "hi\n");
      }
      _ => panic!("expected AddFile hunk"),
    }
  }

  #[test]
  fn test_delete_file_hunk() {
    let result = parse_patch("*** Begin Patch\n*** Delete File: foo.txt\n*** End Patch");
    let parsed = result.expect("should parse");
    assert_eq!(parsed.hunks.len(), 1);
    assert!(matches!(&parsed.hunks[0], DeleteFile { path } if path == &PathBuf::from("foo.txt")));
  }

  #[test]
  fn test_update_file_hunk() {
    let result = parse_patch(
      "*** Begin Patch\n\
             *** Update File: test.py\n\
             @@\n\
             -old\n\
             +new\n\
             *** End Patch",
    );
    let parsed = result.expect("should parse");
    assert_eq!(parsed.hunks.len(), 1);
    match &parsed.hunks[0] {
      UpdateFile { path, chunks, .. } => {
        assert_eq!(path, &PathBuf::from("test.py"));
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].old_lines, vec!["old"]);
        assert_eq!(chunks[0].new_lines, vec!["new"]);
      }
      _ => panic!("expected UpdateFile hunk"),
    }
  }

  #[test]
  fn test_update_file_with_context() {
    let result = parse_patch(
      "*** Begin Patch\n\
             *** Update File: test.py\n\
             @@ def f():\n\
             -    pass\n\
             +    return 123\n\
             *** End Patch",
    );
    let parsed = result.expect("should parse");
    match &parsed.hunks[0] {
      UpdateFile { chunks, .. } => {
        assert_eq!(chunks[0].change_context, Some("def f():".to_string()));
      }
      _ => panic!("expected UpdateFile hunk"),
    }
  }

  #[test]
  fn test_empty_update_hunk_is_error() {
    let result = parse_patch(
      "*** Begin Patch\n\
             *** Update File: test.py\n\
             *** End Patch",
    );
    assert!(result.is_err());
  }

  #[test]
  fn test_multiple_hunks() {
    let result = parse_patch(
      "*** Begin Patch\n\
             *** Add File: path/add.py\n\
             +abc\n\
             +def\n\
             *** Delete File: path/delete.py\n\
             *** Update File: path/update.py\n\
             *** Move to: path/update2.py\n\
             @@ def f():\n\
             -    pass\n\
             +    return 123\n\
             *** End Patch",
    );
    let parsed = result.expect("should parse");
    assert_eq!(parsed.hunks.len(), 3);
    assert!(matches!(&parsed.hunks[0], AddFile { .. }));
    assert!(matches!(&parsed.hunks[1], DeleteFile { .. }));
    assert!(matches!(
      &parsed.hunks[2],
      UpdateFile {
        move_path: Some(_),
        ..
      }
    ));
  }

  #[test]
  fn test_update_without_explicit_context_marker() {
    let result = parse_patch(
      "*** Begin Patch\n\
             *** Update File: file2.py\n \
             import foo\n\
             +bar\n\
             *** End Patch",
    );
    let parsed = result.expect("should parse");
    match &parsed.hunks[0] {
      UpdateFile { chunks, .. } => {
        assert_eq!(chunks[0].change_context, None);
        assert_eq!(chunks[0].old_lines, vec!["import foo"]);
        assert_eq!(chunks[0].new_lines, vec!["import foo", "bar"]);
      }
      _ => panic!("expected UpdateFile"),
    }
  }

  #[test]
  fn test_lenient_heredoc_parsing() {
    let patch_text = "*** Begin Patch\n\
                          *** Add File: foo\n\
                          +hi\n\
                          *** End Patch";
    let heredoc = format!("<<EOF\n{patch_text}\nEOF\n");
    let result = parse_patch(&heredoc);
    let parsed = result.expect("should parse in lenient mode");
    assert_eq!(parsed.hunks.len(), 1);
  }

  #[test]
  fn test_empty_patch_is_ok() {
    let result = parse_patch("*** Begin Patch\n*** End Patch");
    let parsed = result.expect("should parse");
    assert!(parsed.hunks.is_empty());
  }
}
