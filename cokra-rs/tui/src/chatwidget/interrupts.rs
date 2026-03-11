use std::collections::VecDeque;

use cokra_protocol::ExecApprovalRequestEvent;
use cokra_protocol::ExecCommandBeginEvent;
use cokra_protocol::ExecCommandEndEvent;
use cokra_protocol::RequestUserInputEvent;

use super::ChatWidget;
use super::ChatWidgetAction;

#[derive(Debug)]
pub(crate) enum QueuedInterrupt {
  ExecApproval(ExecApprovalRequestEvent),
  RequestUserInput(RequestUserInputEvent),
  ExecBegin(ExecCommandBeginEvent),
  ExecEnd(ExecCommandEndEvent),
}

#[derive(Default)]
pub(crate) struct InterruptManager {
  queue: VecDeque<QueuedInterrupt>,
}

impl InterruptManager {
  #[inline]
  pub(crate) fn is_empty(&self) -> bool {
    self.queue.is_empty()
  }

  pub(crate) fn push_exec_approval(&mut self, ev: ExecApprovalRequestEvent) {
    self.queue.push_back(QueuedInterrupt::ExecApproval(ev));
  }

  pub(crate) fn push_user_input(&mut self, ev: RequestUserInputEvent) {
    self.queue.push_back(QueuedInterrupt::RequestUserInput(ev));
  }

  pub(crate) fn push_exec_begin(&mut self, ev: ExecCommandBeginEvent) {
    self.queue.push_back(QueuedInterrupt::ExecBegin(ev));
  }

  pub(crate) fn push_exec_end(&mut self, ev: ExecCommandEndEvent) {
    self.queue.push_back(QueuedInterrupt::ExecEnd(ev));
  }

  /// Drain all queued interrupts, dispatching each to the appropriate handler.
  /// Returns the last `ChatWidgetAction` produced (approval/user-input prompts).
  pub(crate) fn flush_all(&mut self, chat: &mut ChatWidget) -> Option<ChatWidgetAction> {
    let mut action = None;
    while let Some(q) = self.queue.pop_front() {
      match q {
        QueuedInterrupt::ExecApproval(ev) => {
          action = Some(chat.handle_exec_approval_now(ev));
        }
        QueuedInterrupt::RequestUserInput(ev) => {
          action = Some(ChatWidgetAction::ShowRequestUserInput(ev));
        }
        QueuedInterrupt::ExecBegin(ev) => chat.handle_exec_begin_now(&ev),
        QueuedInterrupt::ExecEnd(ev) => chat.handle_exec_end_now(&ev),
      }
    }
    action
  }
}
