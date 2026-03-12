//! Diff Tracking + Turn Metrics — 追踪工具调用对文件的变更及 Turn 级别统计。
//!
//! 复刻 opencode `snapshot/index.ts` 的 FileDiff 概念，适配 cokra Rust 架构。
//!
//! ## 功能
//! - `TurnMetrics`：记录每次 Turn 的工具调用统计（耗时、成功/失败、可变操作数等）
//! - `FileChangeTracker`：追踪 edit_file/write_file/apply_patch 工具的文件变更
//! - `DiffSummary`：Turn 结束时输出的文件变更摘要（文件数 + 行增减）
//!
//! ## 集成点
//! - `ToolRouter::dispatch_tool_call` 调用前后各记录一次
//! - Turn 结束时通过 `AfterTurn` hook payload 附带 metrics 信息

use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use serde::Deserialize;
use serde::Serialize;

// ── FileDiff ─────────────────────────────────────────────────────────────────

/// 单个文件的 diff 摘要，1:1 opencode FileDiff。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileDiff {
  /// 文件绝对路径。
  pub file: String,
  /// 变更类型。
  pub status: FileChangeStatus,
  /// 新增行数（write/edit 操作估算）。
  pub additions: usize,
  /// 删除行数（edit 操作估算）。
  pub deletions: usize,
}

/// 文件变更类型。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeStatus {
  Added,
  Modified,
  Deleted,
}

// ── ToolCallRecord ────────────────────────────────────────────────────────────

/// 单次工具调用的记录。
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
  pub tool_name: String,
  pub call_id: String,
  pub duration: Duration,
  pub success: bool,
  pub mutating: bool,
}

// ── TurnMetrics ───────────────────────────────────────────────────────────────

/// 一次 Turn 的完整指标汇总。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnMetrics {
  pub turn_id: String,
  /// 工具调用总次数。
  pub tool_calls_total: usize,
  /// 成功次数。
  pub tool_calls_succeeded: usize,
  /// 失败次数。
  pub tool_calls_failed: usize,
  /// 可变操作次数（写文件、执行命令等）。
  pub mutating_calls: usize,
  /// 工具调用总耗时（毫秒）。
  pub total_tool_duration_ms: u64,
  /// 最慢单次工具调用耗时（毫秒）。
  pub slowest_tool_call_ms: u64,
  /// 最慢工具名称。
  pub slowest_tool_name: Option<String>,
  /// 各工具调用次数统计。
  pub tool_call_counts: HashMap<String, usize>,
  /// 文件变更摘要。
  pub file_changes: DiffSummary,
}

impl TurnMetrics {
  pub fn new(turn_id: impl Into<String>) -> Self {
    Self {
      turn_id: turn_id.into(),
      ..Default::default()
    }
  }

  /// 记录一次工具调用完成事件。
  pub fn record_tool_call(&mut self, record: &ToolCallRecord) {
    self.tool_calls_total += 1;
    if record.success {
      self.tool_calls_succeeded += 1;
    } else {
      self.tool_calls_failed += 1;
    }
    if record.mutating {
      self.mutating_calls += 1;
    }

    let ms = record.duration.as_millis() as u64;
    self.total_tool_duration_ms += ms;

    if ms > self.slowest_tool_call_ms {
      self.slowest_tool_call_ms = ms;
      self.slowest_tool_name = Some(record.tool_name.clone());
    }

    *self
      .tool_call_counts
      .entry(record.tool_name.clone())
      .or_insert(0) += 1;
  }

  /// 合并文件变更摘要。
  pub fn merge_diff_summary(&mut self, summary: DiffSummary) {
    self.file_changes = summary;
  }

  /// 输出可读的文字摘要（用于 TUI 的 TurnCompleteHistoryCell）。
  pub fn to_summary_line(&self) -> String {
    let mut parts = Vec::new();

    if self.tool_calls_total > 0 {
      parts.push(format!(
        "{} 工具调用（{}成功/{} 失败）",
        self.tool_calls_total, self.tool_calls_succeeded, self.tool_calls_failed
      ));
    }

    if self.mutating_calls > 0 {
      parts.push(format!("{} 可变操作", self.mutating_calls));
    }

    if self.total_tool_duration_ms > 0 {
      parts.push(format!("耗时 {}ms", self.total_tool_duration_ms));
    }

    let diff = &self.file_changes;
    if diff.files_changed > 0 {
      parts.push(format!(
        "{} 文件变更（+{} -{}）",
        diff.files_changed, diff.total_additions, diff.total_deletions
      ));
    }

    if parts.is_empty() {
      return String::new();
    }

    parts.join(" · ")
  }
}

// ── DiffSummary ───────────────────────────────────────────────────────────────

/// Turn 内文件变更的聚合摘要。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiffSummary {
  /// 变更的文件数量。
  pub files_changed: usize,
  /// 新增文件数。
  pub files_added: usize,
  /// 修改文件数。
  pub files_modified: usize,
  /// 删除文件数。
  pub files_deleted: usize,
  /// 所有文件新增行总数。
  pub total_additions: usize,
  /// 所有文件删除行总数。
  pub total_deletions: usize,
  /// 各文件详细 diff。
  pub file_diffs: Vec<FileDiff>,
}

// ── FileChangeTracker ─────────────────────────────────────────────────────────

/// 文件变更追踪器，在 Turn 生命周期内积累 write/edit/patch 操作的变更信息。
#[derive(Debug, Default)]
pub struct FileChangeTracker {
  /// file_path → FileDiff（同一文件多次变更会合并）
  changes: HashMap<String, FileDiff>,
}

impl FileChangeTracker {
  pub fn new() -> Self {
    Self::default()
  }

  /// 记录一次文件写入（新建或覆写）。
  pub fn record_write(&mut self, file_path: &str, line_count: usize) {
    let is_new = self
      .changes
      .get(file_path)
      .is_none_or(|d| d.status == FileChangeStatus::Added);

    self.changes.insert(
      file_path.to_string(),
      FileDiff {
        file: file_path.to_string(),
        status: if is_new {
          FileChangeStatus::Added
        } else {
          FileChangeStatus::Modified
        },
        additions: line_count,
        deletions: 0,
      },
    );
  }

  /// 记录一次文件编辑（edit_file / apply_patch）。
  ///
  /// `additions` 和 `deletions` 通过对比替换前后内容行数估算。
  pub fn record_edit(&mut self, file_path: &str, old_line_count: usize, new_line_count: usize) {
    let additions = new_line_count.saturating_sub(old_line_count);
    let deletions = old_line_count.saturating_sub(new_line_count);

    let entry = self
      .changes
      .entry(file_path.to_string())
      .or_insert_with(|| FileDiff {
        file: file_path.to_string(),
        status: FileChangeStatus::Modified,
        additions: 0,
        deletions: 0,
      });
    entry.additions += additions;
    entry.deletions += deletions;
    entry.status = FileChangeStatus::Modified;
  }

  /// 记录文件删除。
  pub fn record_delete(&mut self, file_path: &str) {
    self.changes.insert(
      file_path.to_string(),
      FileDiff {
        file: file_path.to_string(),
        status: FileChangeStatus::Deleted,
        additions: 0,
        deletions: 0,
      },
    );
  }

  /// 生成 DiffSummary。
  pub fn to_summary(&self) -> DiffSummary {
    let mut summary = DiffSummary::default();

    for diff in self.changes.values() {
      summary.files_changed += 1;
      summary.total_additions += diff.additions;
      summary.total_deletions += diff.deletions;

      match diff.status {
        FileChangeStatus::Added => summary.files_added += 1,
        FileChangeStatus::Modified => summary.files_modified += 1,
        FileChangeStatus::Deleted => summary.files_deleted += 1,
      }
    }

    let mut diffs: Vec<FileDiff> = self.changes.values().cloned().collect();
    diffs.sort_by(|a, b| a.file.cmp(&b.file));
    summary.file_diffs = diffs;

    summary
  }

  /// 是否有任何记录的变更。
  pub fn is_empty(&self) -> bool {
    self.changes.is_empty()
  }
}

// ── TurnTimer ─────────────────────────────────────────────────────────────────

/// 工具调用计时器，用于测量单次工具调用耗时。
pub struct TurnTimer {
  start: Instant,
}

impl TurnTimer {
  pub fn start() -> Self {
    Self {
      start: Instant::now(),
    }
  }

  pub fn elapsed(&self) -> Duration {
    self.start.elapsed()
  }

  pub fn elapsed_ms(&self) -> u64 {
    self.start.elapsed().as_millis() as u64
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  // ── TurnMetrics 测试 ──────────────────────────────────────────────────

  #[test]
  fn turn_metrics_new_is_zeroed() {
    let m = TurnMetrics::new("t1");
    assert_eq!(m.turn_id, "t1");
    assert_eq!(m.tool_calls_total, 0);
    assert_eq!(m.mutating_calls, 0);
    assert_eq!(m.total_tool_duration_ms, 0);
  }

  #[test]
  fn turn_metrics_record_success() {
    let mut m = TurnMetrics::new("t1");
    m.record_tool_call(&ToolCallRecord {
      tool_name: "edit_file".to_string(),
      call_id: "c1".to_string(),
      duration: Duration::from_millis(120),
      success: true,
      mutating: true,
    });
    assert_eq!(m.tool_calls_total, 1);
    assert_eq!(m.tool_calls_succeeded, 1);
    assert_eq!(m.tool_calls_failed, 0);
    assert_eq!(m.mutating_calls, 1);
    assert_eq!(m.total_tool_duration_ms, 120);
    assert_eq!(m.slowest_tool_name.as_deref(), Some("edit_file"));
  }

  #[test]
  fn turn_metrics_record_failure() {
    let mut m = TurnMetrics::new("t1");
    m.record_tool_call(&ToolCallRecord {
      tool_name: "shell".to_string(),
      call_id: "c2".to_string(),
      duration: Duration::from_millis(50),
      success: false,
      mutating: false,
    });
    assert_eq!(m.tool_calls_failed, 1);
    assert_eq!(m.mutating_calls, 0);
  }

  #[test]
  fn turn_metrics_tracks_slowest_call() {
    let mut m = TurnMetrics::new("t1");
    m.record_tool_call(&ToolCallRecord {
      tool_name: "read_file".to_string(),
      call_id: "c1".to_string(),
      duration: Duration::from_millis(10),
      success: true,
      mutating: false,
    });
    m.record_tool_call(&ToolCallRecord {
      tool_name: "shell".to_string(),
      call_id: "c2".to_string(),
      duration: Duration::from_millis(500),
      success: true,
      mutating: true,
    });
    m.record_tool_call(&ToolCallRecord {
      tool_name: "grep_files".to_string(),
      call_id: "c3".to_string(),
      duration: Duration::from_millis(30),
      success: true,
      mutating: false,
    });

    assert_eq!(m.slowest_tool_call_ms, 500);
    assert_eq!(m.slowest_tool_name.as_deref(), Some("shell"));
    assert_eq!(m.total_tool_duration_ms, 540);
  }

  #[test]
  fn turn_metrics_tool_call_counts() {
    let mut m = TurnMetrics::new("t1");
    for _ in 0..3 {
      m.record_tool_call(&ToolCallRecord {
        tool_name: "read_file".to_string(),
        call_id: "c".to_string(),
        duration: Duration::from_millis(1),
        success: true,
        mutating: false,
      });
    }
    assert_eq!(m.tool_call_counts["read_file"], 3);
  }

  #[test]
  fn turn_metrics_summary_line_with_changes() {
    let mut m = TurnMetrics::new("t1");
    m.record_tool_call(&ToolCallRecord {
      tool_name: "edit_file".to_string(),
      call_id: "c1".to_string(),
      duration: Duration::from_millis(100),
      success: true,
      mutating: true,
    });
    let mut summary = DiffSummary::default();
    summary.files_changed = 2;
    summary.total_additions = 10;
    summary.total_deletions = 3;
    m.merge_diff_summary(summary);

    let line = m.to_summary_line();
    assert!(line.contains("工具调用") || line.contains("tool"));
    assert!(line.contains("文件变更") || line.contains("file"));
  }

  #[test]
  fn turn_metrics_empty_summary_line() {
    let m = TurnMetrics::new("t1");
    assert!(m.to_summary_line().is_empty());
  }

  // ── FileChangeTracker 测试 ────────────────────────────────────────────

  #[test]
  fn tracker_empty_by_default() {
    let tracker = FileChangeTracker::new();
    assert!(tracker.is_empty());
  }

  #[test]
  fn tracker_record_write_marks_as_added() {
    let mut tracker = FileChangeTracker::new();
    tracker.record_write("/tmp/new_file.rs", 50);

    let summary = tracker.to_summary();
    assert_eq!(summary.files_changed, 1);
    assert_eq!(summary.files_added, 1);
    assert_eq!(summary.total_additions, 50);
    assert_eq!(summary.total_deletions, 0);
  }

  #[test]
  fn tracker_record_edit_accumulates() {
    let mut tracker = FileChangeTracker::new();
    tracker.record_edit("/tmp/file.rs", 100, 110);
    tracker.record_edit("/tmp/file.rs", 110, 105);

    let summary = tracker.to_summary();
    assert_eq!(summary.files_changed, 1);
    assert_eq!(summary.files_modified, 1);
    // 第一次 edit: +10 lines, 第二次 edit: -5 lines → total: +10, +0 (saturating)
    // net: additions=10, deletions=5
    assert_eq!(summary.total_additions, 10);
    assert_eq!(summary.total_deletions, 5);
  }

  #[test]
  fn tracker_record_delete() {
    let mut tracker = FileChangeTracker::new();
    tracker.record_delete("/tmp/old.rs");

    let summary = tracker.to_summary();
    assert_eq!(summary.files_deleted, 1);
    assert_eq!(summary.files_changed, 1);
  }

  #[test]
  fn tracker_multiple_files() {
    let mut tracker = FileChangeTracker::new();
    tracker.record_write("/tmp/a.rs", 20);
    tracker.record_edit("/tmp/b.rs", 100, 90);
    tracker.record_delete("/tmp/c.rs");

    let summary = tracker.to_summary();
    assert_eq!(summary.files_changed, 3);
    assert_eq!(summary.files_added, 1);
    assert_eq!(summary.files_modified, 1);
    assert_eq!(summary.files_deleted, 1);
    // file_diffs 按字母顺序
    assert_eq!(summary.file_diffs[0].file, "/tmp/a.rs");
    assert_eq!(summary.file_diffs[1].file, "/tmp/b.rs");
    assert_eq!(summary.file_diffs[2].file, "/tmp/c.rs");
  }

  #[test]
  fn tracker_summary_is_sorted() {
    let mut tracker = FileChangeTracker::new();
    tracker.record_write("/tmp/z.rs", 1);
    tracker.record_write("/tmp/a.rs", 1);
    tracker.record_write("/tmp/m.rs", 1);

    let summary = tracker.to_summary();
    let names: Vec<&str> = summary.file_diffs.iter().map(|d| d.file.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
  }

  // ── TurnTimer 测试 ─────────────────────────────────────────────────────

  #[test]
  fn turn_timer_elapsed_is_non_negative() {
    let timer = TurnTimer::start();
    let ms = timer.elapsed_ms();
    assert!(ms < 1000); // 不应超过 1 秒（单测执行）
  }

  #[test]
  fn turn_timer_elapsed_duration_is_valid() {
    let timer = TurnTimer::start();
    let d = timer.elapsed();
    assert!(d.as_secs() < 2);
  }
}
