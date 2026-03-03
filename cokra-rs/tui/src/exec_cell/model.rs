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
}

impl ExecCell {
  pub(crate) fn new(call: ExecCall, animations_enabled: bool) -> Self {
    Self {
      calls: vec![call],
      animations_enabled,
    }
  }

  pub(crate) fn push_call(&mut self, call: ExecCall) {
    self.calls.push(call);
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

  pub(crate) fn active_start_time(&self) -> Option<Instant> {
    self
      .calls
      .iter()
      .rev()
      .find(|c| c.output.is_none())
      .and_then(|c| c.start_time)
  }

  pub(crate) fn animations_enabled(&self) -> bool {
    self.animations_enabled
  }

  pub(crate) fn iter_calls(&self) -> impl Iterator<Item = &ExecCall> {
    self.calls.iter()
  }
}
