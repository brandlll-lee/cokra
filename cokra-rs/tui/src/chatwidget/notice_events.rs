use super::*;
use crate::multi_agents;
use crate::terminal_palette::light_blue;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;

impl ChatWidget {
  pub(super) fn handle_notice_event(&mut self, event: &EventMsg) -> bool {
    match event {
      EventMsg::CollabAgentSpawnBegin(_) => {}
      EventMsg::CollabAgentSpawnEnd(e) => {
        self.add_to_history(multi_agents::spawn_end(e.clone()));
      }
      EventMsg::CollabAgentInteractionBegin(_) => {}
      EventMsg::CollabAgentInteractionEnd(e) => {
        self.add_to_history(multi_agents::interaction_end(e.clone()));
      }
      EventMsg::ItemStarted(_)
      | EventMsg::ItemCompleted(_)
      | EventMsg::ShutdownComplete
      | EventMsg::ModelReroute(_)
      | EventMsg::AgentReasoningRawContent(_)
      | EventMsg::AgentReasoningRawContentDelta(_)
      | EventMsg::AgentReasoningSectionBreak(_)
      | EventMsg::DynamicToolCallRequest(_)
      | EventMsg::TurnDiff(_)
      | EventMsg::GetHistoryEntryResponse(_)
      | EventMsg::McpListToolsResponse(_)
      | EventMsg::ListCustomPromptsResponse(_)
      | EventMsg::ListSkillsResponse(_)
      | EventMsg::ListRemoteSkillsResponse(_) => {}
      EventMsg::ContextCompacted(_) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(
          "• Context compacted".dim(),
        )]));
      }
      EventMsg::ThreadRolledBack(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Thread rolled back {n} turn{s}",
          n = e.num_turns,
          s = if e.num_turns == 1 { "" } else { "s" },
        ))]));
      }
      EventMsg::AgentReasoningDelta(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
          Span::from("reasoning: ").dim(),
          Span::from(e.delta.clone()).dim(),
        ])]));
      }
      EventMsg::AgentReasoning(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
          Span::from("reasoning: ").dim(),
          Span::from(e.text.clone()).dim(),
        ])]));
      }
      EventMsg::McpStartupUpdate(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• MCP server {}: {:?}",
          e.server, e.status
        ))]));
      }
      EventMsg::McpStartupComplete(e) => {
        if !e.failed.is_empty() {
          for f in &e.failed {
            self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
              Span::from("MCP failed: ").red(),
              Span::from(format!("{}: {}", f.server, f.error)),
            ])]));
          }
        }
      }
      EventMsg::McpToolCallBegin(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Calling {}.{}",
          e.invocation.server, e.invocation.tool
        ))]));
      }
      EventMsg::McpToolCallEnd(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• {}.{} completed ({}ms)",
          e.invocation.server, e.invocation.tool, e.duration_ms
        ))]));
      }
      EventMsg::WebSearchBegin(_) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(
          "• Searching the web...".dim(),
        )]));
      }
      EventMsg::WebSearchEnd(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Searched: {}",
          e.query
        ))]));
      }
      EventMsg::TerminalInteraction(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Terminal input: {}",
          e.stdin.trim()
        ))]));
      }
      EventMsg::ViewImageToolCall(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Viewed image: {}",
          e.path.display()
        ))]));
      }
      EventMsg::ElicitationRequest(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• {} requests input: {}",
          e.server_name, e.message
        ))]));
      }
      EventMsg::ApplyPatchApprovalRequest(e) => {
        let file_count = e.changes.len();
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Patch approval requested ({file_count} file{s})",
          s = if file_count == 1 { "" } else { "s" },
        ))]));
      }
      EventMsg::DeprecationNotice(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(vec![
          Span::from("deprecated: ").red(),
          Span::from(e.summary.clone()),
        ])]));
      }
      EventMsg::BackgroundEvent(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(
          e.message.clone().dim(),
        )]));
      }
      EventMsg::UndoStarted(e) => {
        let msg = e.message.as_deref().unwrap_or("Undoing...");
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!("• {msg}"))]));
      }
      EventMsg::UndoCompleted(e) => {
        let msg = e.message.as_deref().unwrap_or(if e.success {
          "Undo completed"
        } else {
          "Undo failed"
        });
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!("• {msg}"))]));
      }
      EventMsg::PatchApplyBegin(e) => {
        let file_count = e.changes.len();
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Applying patch ({file_count} file{s}){auto}",
          s = if file_count == 1 { "" } else { "s" },
          auto = if e.auto_approved {
            " [auto-approved]"
          } else {
            ""
          },
        ))]));
      }
      EventMsg::PatchApplyEnd(e) => {
        let status_str = if e.success { "succeeded" } else { "failed" };
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Patch apply {status_str}"
        ))]));
      }
      EventMsg::RemoteSkillDownloaded(e) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(format!(
          "• Skill downloaded: {}",
          e.name
        ))]));
      }
      EventMsg::SkillsUpdateAvailable => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(
          Span::from("• Skills update available").style(light_blue()),
        )]));
      }
      EventMsg::PlanUpdate(plan) => {
        let mut lines = Vec::new();
        if let Some(expl) = &plan.explanation {
          lines.push(Line::from(expl.clone().dim()));
        }
        for item in &plan.plan {
          let marker = match item.status {
            cokra_protocol::StepStatus::Completed => "✓",
            cokra_protocol::StepStatus::InProgress => "►",
            cokra_protocol::StepStatus::Pending => "○",
          };
          lines.push(Line::from(format!("  {marker} {}", item.step)));
        }
        self.add_to_history(PlainHistoryCell::new(lines));
      }
      EventMsg::EnteredReviewMode(e) => {
        let hint = e
          .user_facing_hint
          .as_deref()
          .unwrap_or("Entered review mode");
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(
          Span::from(hint.to_string()).style(light_blue()),
        )]));
      }
      EventMsg::ExitedReviewMode(_) => {
        self.add_to_history(PlainHistoryCell::new(vec![Line::from(
          "• Exited review mode".dim(),
        )]));
      }
      EventMsg::ReasoningContentDelta(_) | EventMsg::ReasoningRawContentDelta(_) => {}
      EventMsg::CollabWaitingBegin(e) => {
        // Keep agent-teams progress out of the main transcript to avoid spam when models
        // repeatedly call `wait`. Use the status indicator line as the compact surface.
        self.bottom_pane.ensure_status_indicator();
        if let Some(status) = self.bottom_pane.status_widget_mut() {
          let preview = multi_agents::waiting_preview(e);
          // Tradeoff: inline_message is the only stable "updatable" surface we currently
          // have without introducing editable history cells.
          let inline = if preview.receiver_count > 0 {
            format!("{} agents launched (/agent 展开)", preview.receiver_count)
          } else {
            "Waiting for agents (/agent 展开)".to_string()
          };
          status.update_inline_message(Some(inline));
          if preview.details.is_some() {
            status.update_details(preview.details);
          }
        }
      }
      EventMsg::CollabWaitingEnd(e) => {
        if let Some(status) = self.bottom_pane.status_widget_mut() {
          status.update_inline_message(None);
          // If we set details for agent teams, clear them on completion so exec/tool
          // details do not get stuck behind a finished wait.
          status.update_details(None);
        }

        let fingerprint = wait_end_fingerprint(e);
        if self.session.last_wait_end_fingerprint == Some(fingerprint) {
          return true;
        }
        self.session.last_wait_end_fingerprint = Some(fingerprint);
        self.add_to_history(multi_agents::waiting_end(e.clone()));
      }
      EventMsg::CollabCloseBegin(_) => {}
      EventMsg::CollabCloseEnd(e) => {
        self.add_to_history(multi_agents::close_end(e.clone()));
      }
      EventMsg::CollabMessagePosted(e) => {
        self.add_to_history(multi_agents::message_posted(e.clone()));
      }
      EventMsg::CollabMessagesRead(e) => {
        self.add_to_history(multi_agents::messages_read(e.clone()));
      }
      EventMsg::CollabTaskUpdated(e) => {
        self.add_to_history(multi_agents::task_updated(e.clone()));
      }
      EventMsg::CollabTeamSnapshot(e) => {
        self.add_to_history(multi_agents::team_snapshot(e.clone()));
      }
      EventMsg::CollabPlanSubmitted(e) => {
        self.add_to_history(multi_agents::plan_submitted(e.clone()));
      }
      EventMsg::CollabPlanDecision(e) => {
        self.add_to_history(multi_agents::plan_decision(e.clone()));
      }
      EventMsg::CollabResumeBegin(_) | EventMsg::CollabResumeEnd(_) => {}
      _ => return false,
    }

    true
  }
}

fn wait_end_fingerprint(ev: &cokra_protocol::CollabWaitingEndEvent) -> u64 {
  let mut entries = ev.agent_statuses.clone();
  entries.sort_by(|left, right| left.thread_id.cmp(&right.thread_id));

  let mut hasher = DefaultHasher::new();
  if entries.is_empty() {
    // Defensive fallback: some producers may only fill `statuses`.
    let mut pairs = ev.statuses.iter().collect::<Vec<_>>();
    pairs.sort_by(|(left, _), (right, _)| left.cmp(right));
    for (thread_id, status) in pairs {
      thread_id.hash(&mut hasher);
      hash_agent_status(status, &mut hasher);
    }
  } else {
    for entry in entries {
      entry.thread_id.hash(&mut hasher);
      entry.nickname.hash(&mut hasher);
      entry.role.hash(&mut hasher);
      hash_agent_status(&entry.status, &mut hasher);
    }
  }
  hasher.finish()
}

fn hash_agent_status(status: &cokra_protocol::AgentStatus, hasher: &mut DefaultHasher) {
  match status {
    cokra_protocol::AgentStatus::PendingInit => "PendingInit".hash(hasher),
    cokra_protocol::AgentStatus::Running => "Running".hash(hasher),
    cokra_protocol::AgentStatus::Completed(message) => {
      "Completed".hash(hasher);
      // Tradeoff: hash the full message string so repeated `wait` calls that return the
      // same content dedupe correctly. This can cost some CPU for very large outputs,
      // but keeps UI stable and avoids hiding real changes.
      message.hash(hasher);
    }
    cokra_protocol::AgentStatus::Errored(message) => {
      "Errored".hash(hasher);
      message.hash(hasher);
    }
    cokra_protocol::AgentStatus::Shutdown => "Shutdown".hash(hasher),
    cokra_protocol::AgentStatus::NotFound => "NotFound".hash(hasher),
  }
}
