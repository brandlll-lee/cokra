use cokra_protocol::Op;

use crate::history_cell::HistoryCell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
  Inline,
  AltScreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitMode {
  ShutdownFirst,
  Immediate,
}

#[derive(Debug)]
pub(crate) enum AppEvent {
  CodexOp(Op),
  InsertHistoryCell(Box<dyn HistoryCell>),
  Exit(ExitMode),
  FatalExitRequest(String),
  StartCommitAnimation,
  StopCommitAnimation,
  CommitTick,
  OpenResumePicker,
  NewSession,
  ForkCurrentSession,
}
