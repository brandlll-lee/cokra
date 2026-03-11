//! cokra-file-search
//!
//! Minimal lexical code search for workspace context selection.
//!
//! This is intentionally not a semantic search engine. It provides:
//! - stable, deterministic ranking
//! - line-numbered match snippets
//! - bounded scanning to keep latency predictable

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use ignore::WalkBuilder;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
  pub line: usize,
  pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
  pub path: PathBuf,
  pub score: i64,
  pub matches: Vec<SearchMatch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOutput {
  pub query: String,
  pub root: PathBuf,
  pub truncated: bool,
  pub hits: Vec<SearchHit>,
}

#[derive(Debug, Clone)]
pub struct SearchParams {
  pub root: PathBuf,
  pub query: String,
  pub max_scanned_files: usize,
  pub max_hits: usize,
  pub max_matches_per_file: usize,
  pub max_file_bytes: usize,
}

impl SearchParams {
  pub fn new(root: PathBuf, query: String) -> Self {
    Self {
      root,
      query,
      max_scanned_files: 1500,
      max_hits: 10,
      max_matches_per_file: 8,
      max_file_bytes: 256 * 1024,
    }
  }
}

pub fn search(params: SearchParams) -> anyhow::Result<SearchOutput> {
  let query = params.query.trim().to_string();
  if query.is_empty() {
    return Ok(SearchOutput {
      query,
      root: params.root,
      truncated: false,
      hits: Vec::new(),
    });
  }

  let root = params
    .root
    .canonicalize()
    .unwrap_or_else(|_| params.root.clone());

  let (terms, query_lower) = build_terms(&query);

  let mut scanned_files = 0usize;
  let mut truncated = false;
  let mut hits: Vec<SearchHit> = Vec::new();

  let walker = WalkBuilder::new(&root)
    .standard_filters(true)
    .hidden(false)
    .follow_links(false)
    .build();

  for entry in walker {
    if scanned_files >= params.max_scanned_files {
      truncated = true;
      break;
    }

    let entry = match entry {
      Ok(entry) => entry,
      Err(_) => continue,
    };

    if !entry.file_type().is_some_and(|ft| ft.is_file()) {
      continue;
    }

    let path = entry.path();
    if !should_scan_path(path) {
      continue;
    }

    let meta = match fs::metadata(path) {
      Ok(meta) => meta,
      Err(_) => continue,
    };
    if meta.len() as usize > params.max_file_bytes {
      continue;
    }

    scanned_files += 1;

    let bytes = match fs::read(path) {
      Ok(bytes) => bytes,
      Err(_) => continue,
    };

    let content = String::from_utf8_lossy(&bytes);
    let file_hit = score_file(path, &content, &terms, &query, &query_lower, &params);
    if let Some(hit) = file_hit {
      hits.push(hit);
    }
  }

  hits.sort_by(|a, b| compare_hits(a, b));
  hits.truncate(params.max_hits);

  Ok(SearchOutput {
    query,
    root,
    truncated,
    hits,
  })
}

fn compare_hits(a: &SearchHit, b: &SearchHit) -> Ordering {
  b.score
    .cmp(&a.score)
    .then_with(|| a.path.as_os_str().cmp(b.path.as_os_str()))
}

fn build_terms(query: &str) -> (Vec<String>, String) {
  let mut seen = HashSet::new();
  let mut terms = Vec::new();
  for raw in query.split(|ch: char| ch.is_whitespace() || ch == '/' || ch == '\\') {
    let token = raw.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-');
    if token.len() < 2 {
      continue;
    }
    let lower = token.to_ascii_lowercase();
    if seen.insert(lower.clone()) {
      terms.push(token.to_string());
    }
  }
  let query_lower = query.to_ascii_lowercase();
  (terms, query_lower)
}

fn should_scan_path(path: &Path) -> bool {
  // Keep this list minimal and predictable. Most repos get large wins
  // from skipping these directories.
  if path.components().any(|c| {
    matches!(
      c.as_os_str().to_str(),
      Some("node_modules" | "target" | ".git")
    )
  }) {
    return false;
  }

  // Prefer common source/config extensions. This avoids scanning binaries.
  let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
    return true;
  };
  matches!(
    ext,
    "rs"
      | "toml"
      | "md"
      | "txt"
      | "json"
      | "yml"
      | "yaml"
      | "ts"
      | "tsx"
      | "js"
      | "jsx"
      | "go"
      | "py"
      | "java"
      | "kt"
      | "c"
      | "cc"
      | "cpp"
      | "h"
      | "hpp"
      | "cs"
      | "sh"
      | "bash"
      | "rb"
  )
}

fn score_file(
  path: &Path,
  content: &str,
  terms: &[String],
  _query: &str,
  query_lower: &str,
  params: &SearchParams,
) -> Option<SearchHit> {
  let path_str = path.to_string_lossy();
  let path_lower = path_str.to_ascii_lowercase();

  let mut score: i64 = 0;
  let mut matches: Vec<SearchMatch> = Vec::new();

  if path_lower.contains(query_lower) {
    score += 40;
  }

  for term in terms {
    if path_lower.contains(&term.to_ascii_lowercase()) {
      score += 10;
    }
  }

  for (idx, line) in content.lines().enumerate() {
    if matches.len() >= params.max_matches_per_file {
      break;
    }

    let line_trim = line.trim();
    if line_trim.is_empty() {
      continue;
    }

    let line_lower = line_trim.to_ascii_lowercase();

    let mut line_score = 0i64;
    if line_lower.contains(query_lower) {
      line_score += 50;
    }

    for term in terms {
      let term_lower = term.to_ascii_lowercase();
      if line_lower.contains(&term_lower) {
        line_score += 15;
      }
    }

    if line_score == 0 {
      continue;
    }

    // Earlier matches are more useful for navigation.
    let line_no = idx + 1;
    let proximity = 30i64.saturating_sub((line_no as i64) / 20);
    score += line_score + proximity;

    matches.push(SearchMatch {
      line: line_no,
      text: truncate_line(line_trim, 320),
    });
  }

  if score == 0 || matches.is_empty() {
    return None;
  }

  Some(SearchHit {
    path: path.to_path_buf(),
    score,
    matches,
  })
}

fn truncate_line(text: &str, max_chars: usize) -> String {
  if text.chars().count() <= max_chars {
    return text.to_string();
  }
  let mut out = String::new();
  for ch in text.chars().take(max_chars) {
    out.push(ch);
  }
  out.push_str("...");
  out
}

#[cfg(test)]
mod tests {
  use super::*;
  use anyhow::Context;
  use pretty_assertions::assert_eq;
  use tempfile::tempdir;

  #[test]
  fn search_returns_ranked_hits_with_snippets() -> anyhow::Result<()> {
    let temp = tempdir().context("tempdir")?;
    let root = temp.path();

    fs::write(
      root.join("a.rs"),
      "pub struct ToolRegistry {}\nimpl ToolRegistry { fn new() {} }\n",
    )?;
    fs::write(root.join("b.rs"), "fn spawn_agent() {}\n")?;
    fs::write(
      root.join("notes.txt"),
      "ToolRegistry registered in tools/mod.rs\n",
    )?;

    let out = search(SearchParams {
      root: root.to_path_buf(),
      query: "ToolRegistry".to_string(),
      max_scanned_files: 50,
      max_hits: 10,
      max_matches_per_file: 5,
      max_file_bytes: 64 * 1024,
    })?;

    assert_eq!(out.query, "ToolRegistry".to_string());
    assert!(!out.hits.is_empty());

    let first = &out.hits[0];
    assert!(first.score > 0);
    assert!(!first.matches.is_empty());
    assert!(
      first
        .matches
        .iter()
        .any(|m| m.text.to_ascii_lowercase().contains("toolregistry"))
    );

    Ok(())
  }

  #[test]
  fn search_is_stable_on_ties() -> anyhow::Result<()> {
    let temp = tempdir().context("tempdir")?;
    let root = temp.path();

    fs::write(root.join("a.txt"), "alpha beta\n")?;
    fs::write(root.join("b.txt"), "alpha beta\n")?;

    let out = search(SearchParams {
      root: root.to_path_buf(),
      query: "alpha".to_string(),
      max_scanned_files: 50,
      max_hits: 10,
      max_matches_per_file: 1,
      max_file_bytes: 64 * 1024,
    })?;

    assert_eq!(out.hits.len(), 2);
    let a = out.hits[0].path.file_name().unwrap().to_string_lossy();
    let b = out.hits[1].path.file_name().unwrap().to_string_lossy();
    assert_eq!((a.as_ref(), b.as_ref()), ("a.txt", "b.txt"));
    Ok(())
  }
}
