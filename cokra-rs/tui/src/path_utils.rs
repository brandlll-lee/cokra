//! Path display utilities - 1:1 port from codex-rs/tui/src/diff_render.rs and exec_command.rs
//!
//! This module provides path formatting functions that intelligently convert
//! absolute paths to relative paths for better display in the TUI.

use std::path::Path;
use std::path::PathBuf;

use dirs::home_dir;

/// Return `true` if the project folder is inside a Git repository.
///
/// 1:1 port from codex-rs/core/src/git_info.rs
///
/// The check walks up the directory hierarchy looking for a `.git` file or
/// directory (note `.git` can be a file that contains a `gitdir` entry). This
/// approach does **not** require the `git` binary or the `git2` crate and is
/// therefore fairly lightweight.
pub(crate) fn get_git_repo_root(base_dir: &Path) -> Option<PathBuf> {
  let mut dir = base_dir.to_path_buf();

  loop {
    if dir.join(".git").exists() {
      return Some(dir);
    }

    // Pop one component (go up one directory). `pop` returns false when
    // we have reached the filesystem root.
    if !dir.pop() {
      break;
    }
  }

  None
}

/// Return the current git branch name for `base_dir`, if discoverable.
///
/// This mirrors the lightweight `.git` walking approach above instead of
/// requiring the `git` binary or the `git2` crate.
pub(crate) fn get_git_branch(base_dir: &Path) -> Option<String> {
  let repo_root = get_git_repo_root(base_dir)?;
  let git_entry = repo_root.join(".git");
  let git_dir = if git_entry.is_dir() {
    git_entry
  } else {
    let contents = std::fs::read_to_string(&git_entry).ok()?;
    let gitdir = contents
      .lines()
      .find_map(|line| line.strip_prefix("gitdir:"))
      .map(str::trim)?;
    let git_dir = PathBuf::from(gitdir);
    if git_dir.is_absolute() {
      git_dir
    } else {
      repo_root.join(git_dir)
    }
  };

  let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
  let head = head.trim();
  let reference = head.strip_prefix("ref:")?.trim();
  reference.rsplit('/').next().map(ToString::to_string)
}

/// If `path` is absolute and inside $HOME, return the part *after* the home
/// directory; otherwise, return the path as-is. Note if `path` is the homedir,
/// this will return and empty path.
///
/// 1:1 port from codex-rs/tui/src/exec_command.rs
pub(crate) fn relativize_to_home<P>(path: P) -> Option<PathBuf>
where
  P: AsRef<Path>,
{
  let path = path.as_ref();
  if !path.is_absolute() {
    // If the path is not absolute, we can't do anything with it.
    return None;
  }

  let home_dir = home_dir()?;
  let rel = path.strip_prefix(&home_dir).ok()?;
  Some(rel.to_path_buf())
}

/// Format a path for display relative to the current working directory when
/// possible, keeping output stable in jj/no-`.git` workspaces (e.g. image
/// tool calls should show `example.png` instead of an absolute path).
///
/// 1:1 port from codex-rs/tui/src/diff_render.rs:305-326
///
/// Priority:
/// 1. If path is already relative, return as-is
/// 2. If path is under cwd, return relative path
/// 3. If path and cwd are in the same git repo, return relative path
/// 4. If path is under home directory, return ~/... format
/// 5. Otherwise return absolute path
pub(crate) fn display_path_for(path: &Path, cwd: &Path) -> String {
  // 1. If path is already relative, return as-is
  if path.is_relative() {
    return path.display().to_string();
  }

  // 2. If path is under cwd, return relative path
  if let Ok(stripped) = path.strip_prefix(cwd) {
    return stripped.display().to_string();
  }

  // 3. Check if path and cwd are in the same git repo
  let path_in_same_repo = match (get_git_repo_root(cwd), get_git_repo_root(path)) {
    (Some(cwd_repo), Some(path_repo)) => cwd_repo == path_repo,
    _ => false,
  };

  let chosen = if path_in_same_repo {
    // Use pathdiff to get relative path within the same repo
    pathdiff::diff_paths(path, cwd).unwrap_or_else(|| path.to_path_buf())
  } else {
    // 4. Try to relativize to home directory
    relativize_to_home(path)
      .map(|p| PathBuf::from_iter([Path::new("~"), p.as_path()]))
      .unwrap_or_else(|| path.to_path_buf())
  };

  chosen.display().to_string()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_relativize_to_home() {
    let home = home_dir().expect("home directory");
    let path = home.join("projects").join("myproject");
    let result = relativize_to_home(&path);
    assert_eq!(result, Some(PathBuf::from("projects/myproject")));
  }

  #[test]
  fn test_relativize_to_home_not_in_home() {
    let path = PathBuf::from("/tmp/somefile.txt");
    let result = relativize_to_home(&path);
    // This may or may not be None depending on where /tmp is relative to home
    // Just ensure it doesn't panic
    let _ = result;
  }

  #[test]
  fn test_display_path_for_relative() {
    let cwd = PathBuf::from("/home/user/project");
    let path = PathBuf::from("src/main.rs");
    assert_eq!(display_path_for(&path, &cwd), "src/main.rs");
  }

  #[test]
  fn test_display_path_for_under_cwd() {
    let cwd = PathBuf::from("/home/user/project");
    let path = cwd.join("src").join("main.rs");
    let expected = if cfg!(windows) {
      "src\\main.rs"
    } else {
      "src/main.rs"
    };
    assert_eq!(display_path_for(&path, &cwd), expected);
  }

  #[test]
  fn test_display_path_for_outside_cwd_same_repo() {
    // This test depends on git repo structure, so we just ensure it doesn't panic
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let path = cwd.join("some").join("file.txt");
    let _result = display_path_for(&path, &cwd);
  }

  #[test]
  fn test_get_git_repo_root() {
    // Test from current directory - should find .git if we're in a git repo
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let result = get_git_repo_root(&cwd);
    // Just ensure it doesn't panic - result depends on where test runs
    let _ = result;
  }

  #[test]
  fn test_get_git_branch_from_git_dir() {
    let temp = std::env::temp_dir().join(format!("cokra-branch-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let repo = temp.join("repo");
    let git_dir = repo.join(".git");
    std::fs::create_dir_all(&git_dir).expect("git dir");
    std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/master\n").expect("head");

    assert_eq!(get_git_branch(&repo), Some("master".to_string()));
    let _ = std::fs::remove_dir_all(&temp);
  }

  #[test]
  fn test_get_git_branch_from_git_file_pointer() {
    let temp = std::env::temp_dir().join(format!("cokra-branch-file-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let repo = temp.join("repo");
    let actual_git_dir = temp.join("worktrees").join("repo");
    std::fs::create_dir_all(&actual_git_dir).expect("git dir");
    std::fs::create_dir_all(&repo).expect("repo");
    std::fs::write(
      repo.join(".git"),
      format!("gitdir: {}\n", actual_git_dir.display()),
    )
    .expect("git file");
    std::fs::write(
      actual_git_dir.join("HEAD"),
      "ref: refs/heads/feature/footer\n",
    )
    .expect("head");

    assert_eq!(get_git_branch(&repo), Some("footer".to_string()));
    let _ = std::fs::remove_dir_all(&temp);
  }
}
