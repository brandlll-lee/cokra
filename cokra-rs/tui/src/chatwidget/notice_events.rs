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
        self.add_to_history_preserving_exec(multi_agents::spawn_end(e.clone()));
      }
      EventMsg::CollabAgentInteractionBegin(_) => {}
      EventMsg::CollabAgentInteractionEnd(e) => {
        self.add_to_history_preserving_exec(multi_agents::interaction_end(e.clone()));
      }
      EventMsg::ItemStarted(_)
      | EventMsg::ItemCompleted(_)
      | EventMsg::ShutdownComplete
      | EventMsg::ModelReroute(_)
      | EventMsg::DynamicToolCallRequest(_)
      | EventMsg::TurnDiff(_)
      | EventMsg::GetHistoryEntryResponse(_)
      | EventMsg::McpListToolsResponse(_)
      | EventMsg::ListCustomPromptsResponse(_)
      | EventMsg::ListSkillsResponse(_)
      | EventMsg::ListRemoteSkillsResponse(_) => {}
      EventMsg::ContextCompacted(_) => {
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(
          "● Context compacted".dim(),
        )]));
      }
      EventMsg::ThreadRolledBack(e) => {
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(format!(
          "● Thread rolled back {n} turn{s}",
          n = e.num_turns,
          s = if e.num_turns == 1 { "" } else { "s" },
        ))]));
      }
      EventMsg::AgentReasoningDelta(e) => {
        self.on_reasoning_delta(&e.delta);
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(vec![
          Span::from("reasoning: ").dim(),
          Span::from(e.delta.clone()).dim(),
        ])]));
      }
      EventMsg::AgentReasoning(e) => {
        self.on_reasoning_final(&e.text);
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(vec![
          Span::from("reasoning: ").dim(),
          Span::from(e.text.clone()).dim(),
        ])]));
      }
      EventMsg::AgentReasoningRawContentDelta(e) => {
        self.on_reasoning_delta(&e.delta);
      }
      EventMsg::AgentReasoningRawContent(e) => {
        self.on_reasoning_final(&e.text);
      }
      EventMsg::AgentReasoningSectionBreak(_) => {
        self.on_reasoning_section_break();
      }
      EventMsg::McpStartupUpdate(e) => {
        match &e.status {
          cokra_protocol::McpStartupStatus::Starting => {
            self.session.mcp_starting_servers.insert(e.server.clone());
          }
          cokra_protocol::McpStartupStatus::Ready
          | cokra_protocol::McpStartupStatus::Cancelled
          | cokra_protocol::McpStartupStatus::Failed { .. } => {
            self.session.mcp_starting_servers.remove(&e.server);
          }
        }
        self.sync_status_indicator();
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(format!(
          "● MCP server {}: {:?}",
          e.server, e.status
        ))]));
      }
      EventMsg::McpStartupComplete(e) => {
        self.session.mcp_starting_servers.clear();
        self.sync_status_indicator();
        if !e.failed.is_empty() {
          for f in &e.failed {
            self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(vec![
              Span::from("MCP failed: ").red(),
              Span::from(format!("{}: {}", f.server, f.error)),
            ])]));
          }
        }
      }
      EventMsg::McpToolCallBegin(e) => {
        self.set_status_override(
          format!("Calling {}.{}", e.invocation.server, e.invocation.tool),
          None,
          None,
        );
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(format!(
          "● Calling {}.{}",
          e.invocation.server, e.invocation.tool
        ))]));
      }
      EventMsg::McpToolCallEnd(e) => {
        self.clear_status_override();
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(format!(
          "● {}.{} completed ({}ms)",
          e.invocation.server, e.invocation.tool, e.duration_ms
        ))]));
      }
      EventMsg::WebSearchBegin(_) => {
        self.set_status_override("Searching the web", None, None);
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(
          "● Searching the web...".dim(),
        )]));
      }
      EventMsg::WebSearchEnd(e) => {
        self.clear_status_override();
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(format!(
          "● Searched: {}",
          e.query
        ))]));
      }
      EventMsg::TerminalInteraction(e) => {
        if e.stdin.trim().is_empty() {
          self.set_status_override(
            "Waiting for terminal",
            Some(format!("process {}", e.process_id)),
            None,
          );
        } else {
          self.clear_status_override();
        }
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(format!(
          "● Terminal input: {}",
          e.stdin.trim()
        ))]));
      }
      EventMsg::ViewImageToolCall(e) => {
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(format!(
          "● Viewed image: {}",
          e.path.display()
        ))]));
      }
      EventMsg::ElicitationRequest(e) => {
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(format!(
          "● {} requests input: {}",
          e.server_name, e.message
        ))]));
      }
      EventMsg::ApplyPatchApprovalRequest(e) => {
        let file_count = e.changes.len();
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(format!(
          "● Patch approval requested ({file_count} file{s})",
          s = if file_count == 1 { "" } else { "s" },
        ))]));
      }
      EventMsg::DeprecationNotice(e) => {
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(vec![
          Span::from("deprecated: ").red(),
          Span::from(e.summary.clone()),
        ])]));
      }
      EventMsg::BackgroundEvent(e) => {
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(
          e.message.clone().dim(),
        )]));
      }
      EventMsg::UndoStarted(e) => {
        let msg = e.message.as_deref().unwrap_or("Undoing...");
        self.set_status_override(msg, None, None);
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(format!(
          "● {msg}"
        ))]));
      }
      EventMsg::UndoCompleted(e) => {
        self.clear_status_override();
        let msg = e.message.as_deref().unwrap_or(if e.success {
          "Undo completed"
        } else {
          "Undo failed"
        });
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(format!(
          "● {msg}"
        ))]));
      }
      EventMsg::PatchApplyBegin(e) => {
        let file_count = e.changes.len();
        self.set_status_override(
          format!(
            "Applying patch ({file_count} file{s})",
            s = if file_count == 1 { "" } else { "s" }
          ),
          None,
          None,
        );
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(format!(
          "● Applying patch ({file_count} file{s}){auto}",
          s = if file_count == 1 { "" } else { "s" },
          auto = if e.auto_approved {
            " [auto-approved]"
          } else {
            ""
          },
        ))]));
      }
      EventMsg::PatchApplyEnd(e) => {
        self.clear_status_override();
        let status_str = if e.success { "succeeded" } else { "failed" };
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(format!(
          "● Patch apply {status_str}"
        ))]));
      }
      EventMsg::RemoteSkillDownloaded(e) => {
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(format!(
          "● Skill downloaded: {}",
          e.name
        ))]));
      }
      EventMsg::SkillsUpdateAvailable => {
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(
          Span::from("● Skills update available").style(light_blue()),
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
        self.add_to_history_preserving_exec(PlainHistoryCell::new(lines));
      }
      EventMsg::EnteredReviewMode(e) => {
        let hint = e
          .user_facing_hint
          .as_deref()
          .unwrap_or("Entered review mode");
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(
          Span::from(hint.to_string()).style(light_blue()),
        )]));
      }
      EventMsg::ExitedReviewMode(_) => {
        self.add_to_history_preserving_exec(PlainHistoryCell::new(vec![Line::from(
          "● Exited review mode".dim(),
        )]));
      }
      EventMsg::CollabWaitingBegin(e) => {
        // Keep agent-teams progress out of the main transcript to avoid spam when models
        // repeatedly call `wait`. Use the status indicator line as the compact surface.
        let preview = multi_agents::waiting_preview(e);
        // Tradeoff: inline_message remains the compact expandable hint surface, while
        // the status header now reflects the live "what the model is doing" summary.
        let inline = if preview.receiver_count > 0 {
          format!(
            "{} agents launched (/agent to expand)",
            preview.receiver_count
          )
        } else {
          "Waiting for agents (/agent to expand)".to_string()
        };
        self.set_collab_wait_status(Some(StatusSnapshot::new(
          preview.summary,
          preview.details,
          Some(inline),
        )));
      }
      EventMsg::CollabWaitingEnd(e) => {
        self.set_collab_wait_status(None);

        let fingerprint = wait_end_fingerprint(e);
        if self.session.last_wait_end_fingerprint == Some(fingerprint) {
          return true;
        }
        self.session.last_wait_end_fingerprint = Some(fingerprint);
        self.add_to_history_preserving_exec(multi_agents::waiting_end(e.clone()));
      }
      EventMsg::CollabCloseBegin(_) => {}
      EventMsg::CollabCloseEnd(e) => {
        self.add_to_history_preserving_exec(multi_agents::close_end(e.clone()));
      }
      EventMsg::CollabMessagePosted(e) => {
        self.add_to_history_preserving_exec(multi_agents::message_posted(e.clone()));
      }
      EventMsg::CollabMessagesRead(e) => {
        self.add_to_history_preserving_exec(multi_agents::messages_read(e.clone()));
      }
      EventMsg::CollabTaskUpdated(e) => {
        self.add_to_history_preserving_exec(multi_agents::task_updated(e.clone()));
      }
      EventMsg::CollabTeamSnapshot(e) => {
        self.add_to_history_preserving_exec(multi_agents::team_snapshot(e.clone()));
      }
      EventMsg::CollabPlanSubmitted(e) => {
        self.add_to_history_preserving_exec(multi_agents::plan_submitted(e.clone()));
      }
      EventMsg::CollabPlanDecision(e) => {
        self.add_to_history_preserving_exec(multi_agents::plan_decision(e.clone()));
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

#[cfg(test)]
mod tests {
  use super::*;
  use crate::app_event_sender::AppEventSender;
  use crate::tui::FrameRequester;
  use tokio::sync::mpsc::unbounded_channel;

  fn make_widget() -> ChatWidget {
    let (tx, _rx) = unbounded_channel();
    let sender = AppEventSender::new(tx);
    let mut widget = ChatWidget::new(
      sender,
      FrameRequester::test_dummy(),
      false,
      StreamRenderMode::AnimatedPreview,
    );
    widget.set_agent_turn_running(true);
    widget
  }

  #[test]
  fn reasoning_delta_replaces_working_header() {
    let mut widget = make_widget();

    widget.handle_notice_event(&EventMsg::AgentReasoningDelta(
      cokra_protocol::AgentReasoningDeltaEvent {
        delta: "**Analyzing CLI rendering** collecting context".to_string(),
      },
    ));

    let status = widget
      .bottom_pane
      .status_widget()
      .expect("status should stay visible while turn is active");
    assert_eq!(status.header(), "Analyzing CLI rendering");
  }

  #[test]
  fn collab_waiting_uses_live_wait_summary_and_restores_working() {
    let mut widget = make_widget();

    widget.handle_notice_event(&EventMsg::CollabWaitingBegin(
      cokra_protocol::CollabWaitingBeginEvent {
        sender_thread_id: "main".to_string(),
        receiver_thread_ids: Vec::new(),
        receiver_agents: vec![
          cokra_protocol::CollabAgentRef {
            thread_id: "agent-1".to_string(),
            nickname: Some("有村架纯".to_string()),
            role: None,
          },
          cokra_protocol::CollabAgentRef {
            thread_id: "agent-2".to_string(),
            nickname: Some("菅田将晖".to_string()),
            role: None,
          },
        ],
        call_id: "wait-1".to_string(),
      },
    ));

    let status = widget
      .bottom_pane
      .status_widget()
      .expect("status should be visible during collab wait");
    assert_eq!(status.header(), "Waiting for 2 agents");
    assert_eq!(
      status.inline_message(),
      Some("2 agents launched (/agent to expand)")
    );
    let details = status
      .details()
      .expect("multi-agent wait should show details");
    assert!(details.contains("@有村架纯"));
    assert!(details.contains("@菅田将晖"));

    widget.handle_notice_event(&EventMsg::CollabWaitingEnd(
      cokra_protocol::CollabWaitingEndEvent {
        sender_thread_id: "main".to_string(),
        call_id: "wait-1".to_string(),
        agent_statuses: Vec::new(),
        statuses: std::collections::HashMap::new(),
      },
    ));

    let status = widget
      .bottom_pane
      .status_widget()
      .expect("status should remain visible while turn is active");
    assert_eq!(status.header(), "Working");
    assert_eq!(status.inline_message(), None);
    assert_eq!(status.details(), None);
  }

  #[test]
  fn reasoning_delta_does_not_flush_active_exec_cell() {
    let mut widget = make_widget();

    widget.handle_exec_begin_now(&cokra_protocol::ExecCommandBeginEvent {
      thread_id: "thread-1".to_string(),
      turn_id: "turn-1".to_string(),
      command_id: "call-1".to_string(),
      tool_name: "code_search".to_string(),
      command: "agentteams".to_string(),
      cwd: std::path::PathBuf::from("/tmp/project"),
    });

    widget.handle_notice_event(&EventMsg::AgentReasoningDelta(
      cokra_protocol::AgentReasoningDeltaEvent {
        delta: "**Investigating agent teams** collecting references".to_string(),
      },
    ));

    let cell = widget
      .transcript
      .active_exec_cell
      .as_ref()
      .and_then(|c| c.as_any().downcast_ref::<crate::exec_cell::ExecCell>())
      .expect("active exec cell should remain visible");
    assert_eq!(cell.calls.len(), 1);
    assert!(cell.is_active());
  }
}
