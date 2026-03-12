use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

#[derive(Clone, Debug, Default)]
pub(crate) struct CommandOutput {
  pub(crate) exit_code: i32,
  /// Aggregated stdout/stderr chunks in arrival order.
  pub(crate) output: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ExecCall {
  pub(crate) command_id: String,
  /// The actual tool name (e.g. "shell", "read_file", "list_dir").
  pub(crate) tool_name: String,
  /// For shell: the raw command string. For other tools: same as tool_name.
  pub(crate) command: String,
  pub(crate) cwd: PathBuf,
  pub(crate) output: Option<CommandOutput>,
  pub(crate) start_time: Option<Instant>,
  pub(crate) duration: Option<Duration>,
}

#[derive(Debug)]
pub(crate) struct ExecCell {
  pub(crate) calls: Vec<ExecCall>,
  animations_enabled: bool,
  /// When true, `HistoryCell::is_stream_continuation()` returns true so that
  /// `prepare_history_display` suppresses the blank separator line. Used when
  /// an exploring cell snapshot is written to scrollback as a progressive
  /// update (not the very first line of a new logical group).
  pub(crate) is_continuation: bool,
  /// The wall-clock time when this exploring group was first created.
  /// Used to drive a *continuous* spinner for the entire lifetime of the
  /// group while it lives in `active_exec_cell`, regardless of whether any
  /// individual call is currently in-flight (calls arrive sequentially, so
  /// there are brief gaps between exec_end and the next exec_begin where
  /// `is_active()` is false but the group is still open).
  /// `None` for non-exploring cells and for scrollback snapshots.
  pub(crate) exploring_since: Option<Instant>,
  /// Wall-clock time when this exploring cell was first placed into
  /// `active_exec_cell`. Used by the TUI to enforce a minimum visibility
  /// window so that fast exploring tools (e.g. read_file <1ms) still show
  /// the "⠋ Exploring" state for at least a brief moment before being
  /// flushed to scrollback.
  pub(crate) exploring_visible_since: Option<Instant>,
}

impl ExecCell {
  pub(crate) fn new(call: ExecCall, animations_enabled: bool) -> Self {
    let exploring_since = Self::is_exploring_call(&call).then(Instant::now);
    Self {
      calls: vec![call],
      animations_enabled,
      is_continuation: false,
      exploring_since,
      exploring_visible_since: exploring_since,
    }
  }

  /// Return a completed-snapshot clone of self suitable for writing to scrollback
  /// as a progressive exploring update. The clone is marked as a stream
  /// continuation so no blank separator line is inserted before it.
  /// Snapshots have `exploring_since = None` so they always render as
  /// "● Explored" (no spinner).
  pub(crate) fn scrollback_snapshot(&self) -> Self {
    Self {
      calls: self.calls.clone(),
      animations_enabled: self.animations_enabled,
      is_continuation: true,
      exploring_since: None,
      exploring_visible_since: None,
    }
  }

  pub(crate) fn push_call(&mut self, call: ExecCall) {
    self.calls.push(call);
  }

  pub(crate) fn with_added_call(&self, call: ExecCall) -> Option<Self> {
    if self.is_exploring_cell() && Self::is_exploring_call(&call) {
      let mut calls = self.calls.clone();
      calls.push(call);
      Some(Self {
        calls,
        animations_enabled: self.animations_enabled,
        is_continuation: self.is_continuation,
        exploring_since: self.exploring_since,
        exploring_visible_since: self.exploring_visible_since,
      })
    } else {
      None
    }
  }

  pub(crate) fn complete_call(
    &mut self,
    command_id: &str,
    output: CommandOutput,
    duration: Duration,
  ) {
    if let Some(call) = self
      .calls
      .iter_mut()
      .rev()
      .find(|c| c.command_id == command_id)
    {
      call.output = Some(output);
      call.duration = Some(duration);
      call.start_time = None;
    }
  }

  pub(crate) fn append_output(&mut self, command_id: &str, chunk: &str) -> bool {
    if chunk.is_empty() {
      return false;
    }

    let Some(call) = self
      .calls
      .iter_mut()
      .rev()
      .find(|c| c.command_id == command_id)
    else {
      return false;
    };

    let output = call.output.get_or_insert_with(CommandOutput::default);
    output.output.push_str(chunk);
    true
  }

  pub(crate) fn mark_failed_incomplete(&mut self) {
    for call in &mut self.calls {
      if call.output.is_some() {
        continue;
      }
      let elapsed = call.start_time.map(|st| st.elapsed()).unwrap_or_default();
      call.start_time = None;
      call.duration = Some(elapsed);
      call.output = Some(CommandOutput {
        exit_code: 1,
        output: String::new(),
      });
    }
  }

  pub(crate) fn is_active(&self) -> bool {
    self.calls.iter().any(|c| c.output.is_none())
  }

  pub(crate) fn should_flush(&self) -> bool {
    !self.is_exploring_cell() && self.calls.iter().all(|c| c.output.is_some())
  }

  pub(crate) fn active_start_time(&self) -> Option<Instant> {
    self
      .calls
      .iter()
      .rev()
      .find(|c| c.output.is_none())
      .and_then(|c| c.start_time)
  }

  pub(crate) fn active_call(&self) -> Option<&ExecCall> {
    self.calls.iter().rev().find(|c| c.output.is_none())
  }

  pub(crate) fn animations_enabled(&self) -> bool {
    self.animations_enabled
  }

  pub(crate) fn iter_calls(&self) -> impl Iterator<Item = &ExecCall> {
    self.calls.iter()
  }

  pub(crate) fn is_exploring_cell(&self) -> bool {
    self.calls.iter().all(Self::is_exploring_call)
  }

  /// Returns the current spinner animation frame tick for this exploring cell.
  /// When `exploring_since` is set, the tick advances every `SPINNER_INTERVAL_MS`
  /// milliseconds so that callers can use it as a cache-key to schedule redraws.
  pub(crate) fn exploring_animation_tick(&self) -> Option<u64> {
    let since = self.exploring_since?;
    let elapsed_ms = since.elapsed().as_millis();
    Some((elapsed_ms / crate::exec_cell::SPINNER_INTERVAL_MS) as u64)
  }

  pub(crate) fn is_exploring_call(call: &ExecCall) -> bool {
    matches!(
      call.tool_name.as_str(),
      "read_file"
        | "list_dir"
        | "grep_files"
        | "search_tool"
        | "code_search"
        | "glob"
        | "read_many_files"
    )
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn exploring_call(command_id: &str, tool_name: &str, command: &str, active: bool) -> ExecCall {
    ExecCall {
      command_id: command_id.to_string(),
      tool_name: tool_name.to_string(),
      command: command.to_string(),
      cwd: PathBuf::from("."),
      output: if active {
        None
      } else {
        Some(CommandOutput {
          exit_code: 0,
          output: String::new(),
        })
      },
      start_time: active.then(Instant::now),
      duration: (!active).then(|| Duration::from_millis(1)),
    }
  }

  #[test]
  fn exploring_exec_cells_keep_merging_until_flushed() {
    let active = ExecCell::new(exploring_call("c1", "list_dir", "cokra-rs", true), false);
    assert!(
      active
        .with_added_call(exploring_call(
          "c2",
          "read_file",
          "PROJECT_STRUCTURE.md",
          true
        ))
        .is_some(),
      "active exploring group should merge additional exploring calls"
    );

    let inactive = ExecCell::new(exploring_call("c1", "list_dir", "cokra-rs", false), false);
    assert!(
      inactive
        .with_added_call(exploring_call(
          "c2",
          "read_file",
          "PROJECT_STRUCTURE.md",
          true
        ))
        .is_some(),
      "completed exploring groups should keep coalescing until the transcript flushes them"
    );
  }

  #[test]
  fn glob_is_treated_as_exploring() {
    let cell = ExecCell::new(exploring_call("c1", "glob", "**/*.rs", true), false);
    assert!(
      cell.is_exploring_cell(),
      "glob should be grouped into the Explored cell"
    );
  }

  #[test]
  fn read_many_files_is_treated_as_exploring() {
    let cell = ExecCell::new(
      exploring_call("c1", "read_many_files", "src/main.rs,src/lib.rs", true),
      false,
    );
    assert!(
      cell.is_exploring_cell(),
      "read_many_files should be grouped into the Explored cell"
    );
  }

  #[test]
  fn glob_merges_with_other_exploring_calls() {
    let active = ExecCell::new(exploring_call("c1", "glob", "**/*.rs", true), false);
    assert!(
      active
        .with_added_call(exploring_call("c2", "read_many_files", "src/lib.rs", true))
        .is_some(),
      "glob and read_many_files should merge into the same Exploring group"
    );
  }

  #[test]
  fn code_search_is_treated_as_exploring() {
    let cell = ExecCell::new(
      exploring_call("c1", "code_search", "spawn_agent", true),
      false,
    );

    assert!(cell.is_exploring_cell());
  }
}
