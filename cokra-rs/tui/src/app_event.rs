use cokra_protocol::ExecApprovalRequestEvent;
use cokra_protocol::Op;
use cokra_protocol::ReasoningEffortConfig;
use cokra_protocol::RequestUserInputEvent;

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
  SelectAgentThread {
    thread_id: String,
  },
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
  OpenBackgroundApproval(ExecApprovalRequestEvent),
  OpenBackgroundUserInput(RequestUserInputEvent),

  OpenAllModelsPopup {
    providers: Vec<ProviderInfo>,
  },
  OpenModelRootPopup,
  OpenAvailableModelsPopup,
  OpenConnectProvidersPopup,
  OpenConnectProviderDetail {
    provider: ProviderInfo,
  },
  StartOAuthConnect {
    provider_id: String,
  },
  CancelOAuthConnect {
    provider_id: String,
  },
  DismissBottomPaneView,
  DisconnectProvider {
    provider_id: String,
  },
  OpenReasoningPopup {
    model_id: String,
  },
  OpenApiKeyEntry {
    provider_id: String,
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
  OAuthCodeSubmitted {
    provider_id: String,
    input: String,
  },
  OAuthCompleted {
    provider_id: String,
    stored: cokra_core::model::auth::StoredCredentials,
  },
  OAuthFailed {
    provider_id: String,
    message: String,
  },
}
