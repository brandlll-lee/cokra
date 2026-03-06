use super::*;
use crate::multi_agents;
use crate::terminal_palette::light_blue;

impl ChatWidget {
  pub(super) fn handle_notice_event(&mut self, event: &EventMsg) -> bool {
    match event {
      EventMsg::CollabAgentSpawnBegin(e) => {
        self.add_to_history(multi_agents::spawn_begin(e.clone()));
      }
      EventMsg::CollabAgentSpawnEnd(e) => {
        self.add_to_history(multi_agents::spawn_end(e.clone()));
      }
      EventMsg::CollabAgentInteractionBegin(e) => {
        self.add_to_history(multi_agents::interaction_begin(e.clone()));
      }
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
      EventMsg::CollabWaitingBegin(_) => {
        if let EventMsg::CollabWaitingBegin(e) = event {
          self.add_to_history(multi_agents::waiting_begin(e.clone()));
        }
      }
      EventMsg::CollabWaitingEnd(_) => {
        if let EventMsg::CollabWaitingEnd(e) = event {
          self.add_to_history(multi_agents::waiting_end(e.clone()));
        }
      }
      EventMsg::CollabCloseBegin(e) => {
        self.add_to_history(multi_agents::close_begin(e.clone()));
      }
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
