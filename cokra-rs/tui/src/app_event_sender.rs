use tokio::sync::mpsc::UnboundedSender;

use crate::app_event::AppEvent;
use crate::app_event::ExitMode;
use crate::history_cell::HistoryCell;

#[derive(Clone, Debug)]
pub(crate) struct AppEventSender {
  pub app_event_tx: UnboundedSender<AppEvent>,
}

impl AppEventSender {
  pub(crate) fn new(app_event_tx: UnboundedSender<AppEvent>) -> Self {
    Self { app_event_tx }
  }

  /// Send an event to the app event channel. If it fails, we swallow the
  /// error and log it.
  pub(crate) fn send(&self, event: AppEvent) {
    if let Err(e) = self.app_event_tx.send(event) {
      tracing::error!("failed to send event: {e}");
    }
  }

  pub(crate) fn insert_history_cell(&self, cell: impl HistoryCell + 'static) {
    self.send(AppEvent::InsertHistoryCell(Box::new(cell)));
  }

  pub(crate) fn insert_boxed_history_cell(&self, cell: Box<dyn HistoryCell>) {
    self.send(AppEvent::InsertHistoryCell(cell));
  }

  pub(crate) fn fatal_exit(&self, message: impl Into<String>) {
    self.send(AppEvent::FatalExitRequest(message.into()));
  }

  pub(crate) fn request_exit(&self, mode: ExitMode) {
    self.send(AppEvent::Exit(mode));
  }
}
