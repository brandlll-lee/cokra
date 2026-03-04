use cokra_protocol::Op;
use cokra_protocol::ReasoningEffortConfig;

use cokra_core::model::ProviderInfo;

use crate::history_cell::HistoryCell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
  Inline,
  AltScreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusLineMode {
  Default,
  Minimal,
  Off,
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
  SetStatusLineMode(StatusLineMode),

  OpenAllModelsPopup {
    providers: Vec<ProviderInfo>,
  },
  OpenReasoningPopup {
    model_id: String,
  },
  ApplyModelSelection {
    model_id: String,
    effort: Option<ReasoningEffortConfig>,
  },

  ApiKeySubmitted {
    provider_id: String,
    api_key: String,
    model_id: String,
    effort: Option<ReasoningEffortConfig>,
  },
}
