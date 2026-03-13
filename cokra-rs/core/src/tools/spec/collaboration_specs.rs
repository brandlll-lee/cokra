use std::collections::BTreeMap;

use super::JsonSchema;
use super::ToolHandlerType;
use super::ToolSourceKind;
use super::ToolSpec;
use super::bool_field;
use super::default_permissions;
use super::int_field;
use super::obj;
use super::str_field;

pub(crate) fn build_specs() -> Vec<ToolSpec> {
  vec![
    spawn_agent_tool(),
    send_input_tool(),
    wait_tool(),
    close_agent_tool(),
    assign_team_task_tool(),
    claim_team_task_tool(),
    claim_team_messages_tool(),
    handoff_team_task_tool(),
    cleanup_team_tool(),
    submit_team_plan_tool(),
    approve_team_plan_tool(),
    team_status_tool(),
    send_team_message_tool(),
    read_team_messages_tool(),
    create_team_task_tool(),
    update_team_task_tool(),
  ]
}

fn collaboration_tool(
  name: &str,
  description: impl Into<String>,
  input_schema: JsonSchema,
) -> ToolSpec {
  ToolSpec::new(
    name,
    description,
    input_schema,
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
  .with_source_kind(ToolSourceKind::BuiltinCollaboration)
  .with_permission_key("team")
  .with_supports_parallel(false)
  .with_mutates_state(true)
}

fn spawn_agent_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "task".to_string(),
    str_field("Initial task text for the spawned agent."),
  );
  props.insert(
    "message".to_string(),
    str_field("Alias of task for Codex-style compatibility."),
  );
  props.insert(
    "nickname".to_string(),
    str_field("Optional human-readable teammate name shown in team UI."),
  );
  props.insert("role".to_string(), str_field("Agent role."));
  props.insert(
    "agent_type".to_string(),
    str_field("Alias of role for Codex-style compatibility."),
  );
  collaboration_tool(
    "spawn_agent",
    "Spawn a sub-agent and immediately start it on an initial task.",
    obj(props, &[]),
  )
  .with_permission_key("agent")
}

fn send_input_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "agent_id".to_string(),
    str_field("Target spawned agent id."),
  );
  props.insert(
    "message".to_string(),
    str_field("New message to send to the spawned agent."),
  );
  collaboration_tool(
    "send_input",
    "Send another message to a running or completed spawned agent.",
    obj(props, &["agent_id", "message"]),
  )
  .with_permission_key("agent")
}

fn wait_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "agent_ids".to_string(),
    JsonSchema::Array {
      items: Box::new(str_field("Spawned agent id.")),
      description: Some(
        "Optional spawned agent ids to wait on. Defaults to all known spawned agents.".to_string(),
      ),
    },
  );
  props.insert(
    "timeout_ms".to_string(),
    int_field("Optional wait timeout in milliseconds."),
  );
  collaboration_tool(
    "wait",
    "Wait for spawned agents to finish before continuing.",
    obj(props, &[]),
  )
  .with_permission_key("agent")
  .with_mutates_state(false)
}

fn close_agent_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "agent_id".to_string(),
    str_field("Target spawned agent id."),
  );
  collaboration_tool(
    "close_agent",
    "Close and clean up a spawned agent when it is no longer needed.",
    obj(props, &["agent_id"]),
  )
  .with_permission_key("agent")
}

fn assign_team_task_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("task_id".to_string(), str_field("Task id to assign."));
  props.insert(
    "assignee_thread_id".to_string(),
    str_field("Thread id of the teammate who should own the task."),
  );
  props.insert("note".to_string(), str_field("Optional assignment note."));
  collaboration_tool(
    "assign_team_task",
    "Assign a shared team task to a specific teammate.",
    obj(props, &["task_id", "assignee_thread_id"]),
  )
}

fn claim_team_task_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("task_id".to_string(), str_field("Task id to claim."));
  props.insert(
    "note".to_string(),
    str_field("Optional claim note to append to the task history."),
  );
  collaboration_tool(
    "claim_team_task",
    "Claim a shared team task for the current teammate and mark it in progress.",
    obj(props, &["task_id"]),
  )
}

fn claim_team_messages_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "queue_name".to_string(),
    str_field("Queue name to claim messages from."),
  );
  props.insert(
    "limit".to_string(),
    int_field("Maximum number of queue messages to claim."),
  );
  collaboration_tool(
    "claim_team_messages",
    "Claim work items from a shared team mailbox queue.",
    obj(props, &["queue_name"]),
  )
}

fn handoff_team_task_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("task_id".to_string(), str_field("Task id to hand off."));
  props.insert(
    "to_thread_id".to_string(),
    str_field("Teammate thread id receiving the task."),
  );
  props.insert("note".to_string(), str_field("Optional handoff note."));
  props.insert(
    "review_mode".to_string(),
    bool_field("When true, hand off the task in review mode instead of pending mode."),
  );
  collaboration_tool(
    "handoff_team_task",
    "Hand off a task to another teammate, optionally marking it ready for review.",
    obj(props, &["task_id", "to_thread_id"]),
  )
}

fn cleanup_team_tool() -> ToolSpec {
  collaboration_tool(
    "cleanup_team",
    "Close all spawned agents and clear persisted team mailbox and task state for this workspace.",
    obj(BTreeMap::new(), &[]),
  )
}

fn submit_team_plan_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "summary".to_string(),
    str_field("Short summary of the proposed plan."),
  );
  props.insert(
    "steps".to_string(),
    JsonSchema::Array {
      items: Box::new(str_field("Plan step.")),
      description: Some("Ordered plan steps.".to_string()),
    },
  );
  props.insert(
    "requires_approval".to_string(),
    bool_field("Whether this teammate must wait for approval before mutating work."),
  );
  collaboration_tool(
    "submit_team_plan",
    "Submit a teammate work plan for approval before making mutating changes.",
    obj(props, &["summary", "steps"]),
  )
  .with_permission_key("plan")
}

fn approve_team_plan_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "plan_id".to_string(),
    str_field("Plan id to approve or reject."),
  );
  props.insert(
    "approved".to_string(),
    bool_field("Whether to approve the plan."),
  );
  props.insert("note".to_string(), str_field("Optional reviewer note."));
  collaboration_tool(
    "approve_team_plan",
    "Approve or reject a teammate's submitted work plan.",
    obj(props, &["plan_id", "approved"]),
  )
  .with_permission_key("plan")
}

fn team_status_tool() -> ToolSpec {
  collaboration_tool(
    "team_status",
    "Return the shared team snapshot, including members, tasks, plans, unread mailbox counts, and workflow runtime state.",
    obj(BTreeMap::new(), &[]),
  )
  .with_supports_parallel(true)
  .with_mutates_state(false)
}

fn send_team_message_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("message".to_string(), str_field("Message body to send."));
  props.insert(
    "recipient_thread_id".to_string(),
    str_field("Optional teammate thread id. Omit to broadcast to the whole team."),
  );
  collaboration_tool(
    "send_team_message",
    "Send a direct or broadcast team mailbox message.",
    obj(props, &["message"]),
  )
}

fn read_team_messages_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "unread_only".to_string(),
    bool_field("When true, only return unread mailbox messages."),
  );
  collaboration_tool(
    "read_team_messages",
    "Read your team mailbox messages and mark them as seen.",
    obj(props, &[]),
  )
  .with_supports_parallel(true)
}

fn create_team_task_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("title".to_string(), str_field("Short team task title."));
  props.insert(
    "details".to_string(),
    str_field("Optional detailed task description."),
  );
  props.insert(
    "assignee_thread_id".to_string(),
    str_field("Optional teammate thread id to assign immediately."),
  );
  props.insert(
    "workflow_run_id".to_string(),
    str_field("Optional workflow run id to link this task back to a resumable workflow."),
  );
  collaboration_tool(
    "create_team_task",
    "Create a shared team task on the common task board, optionally linked to a workflow run.",
    obj(props, &["title"]),
  )
}

fn update_team_task_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("task_id".to_string(), str_field("Task id to update."));
  props.insert(
    "status".to_string(),
    JsonSchema::String {
      description: Some(
        "Optional new task status: Pending, InProgress, Completed, Failed, or Canceled."
          .to_string(),
      ),
    },
  );
  props.insert(
    "assignee_thread_id".to_string(),
    str_field("Optional new assignee thread id."),
  );
  props.insert(
    "clear_assignee".to_string(),
    bool_field("When true, clears the current assignee."),
  );
  props.insert(
    "note".to_string(),
    str_field("Optional note to append to the task history."),
  );
  collaboration_tool(
    "update_team_task",
    "Update a shared team task status, assignee, or notes.",
    obj(props, &["task_id"]),
  )
}
