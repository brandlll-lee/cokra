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
    send_team_nudge_tool(),
    ack_team_message_tool(),
    watch_team_inbox_tool(),
    read_team_messages_tool(),
    create_team_task_tool(),
    update_team_task_tool(),
    release_task_leases_tool(),
    force_release_lease_tool(),
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

fn scope_request_schema(description: &str) -> JsonSchema {
  let mut props = BTreeMap::new();
  props.insert(
    "kind".to_string(),
    str_field("Ownership scope kind: File, Directory, Glob, or Module."),
  );
  props.insert(
    "path".to_string(),
    str_field("Scope path or module identifier."),
  );
  props.insert(
    "access".to_string(),
    str_field("Requested access mode: SharedRead, ExclusiveWrite, or Review."),
  );
  props.insert(
    "reason".to_string(),
    str_field("Optional reason for requesting this scope."),
  );
  JsonSchema::Array {
    items: Box::new(obj(props, &["path"])),
    description: Some(description.to_string()),
  }
}

fn spawn_agent_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "task".to_string(),
    str_field("Initial task text for the spawned agent."),
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
    str_field("Target spawned agent selector: thread id, nickname, or @nickname."),
  );
  props.insert(
    "message".to_string(),
    str_field("New message to send to the spawned agent."),
  );
  collaboration_tool(
    "send_input",
    "Send another message to a teammate using a thread id, nickname, or @nickname.",
    obj(props, &["agent_id", "message"]),
  )
  .with_permission_key("agent")
}

fn wait_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "agent_ids".to_string(),
    JsonSchema::Array {
      items: Box::new(str_field(
        "Spawned agent selector: thread id, nickname, or @nickname.",
      )),
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
    "Wait for spawned agents to settle their currently scheduled work batch before continuing.",
    obj(props, &[]),
  )
  .with_permission_key("agent")
  .with_mutates_state(false)
}

fn close_agent_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "agent_id".to_string(),
    str_field("Target spawned agent selector: thread id, nickname, or @nickname."),
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
    str_field("Teammate selector receiving the task: thread id, nickname, or @nickname."),
  );
  props.insert("note".to_string(), str_field("Optional assignment note."));
  props.insert(
    "override_assignee".to_string(),
    bool_field("When true, explicitly reassigns a task away from its current assignee."),
  );
  collaboration_tool(
    "assign_team_task",
    "Assign a shared team task to a specific teammate without auto-claiming it.",
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
    "Compatibility wrapper that claims a ready shared team task for the current teammate and marks it in progress.",
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
    str_field("Teammate selector receiving the task: thread id, nickname, or @nickname."),
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
    "Return the shared team snapshot, including members, tasks, plans, unread mailbox counts, approvals, artifacts, and resumable run state.",
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
    str_field("Optional teammate selector. Accepts thread id, nickname, or @nickname. Omit to broadcast to the whole team."),
  );
  props.insert(
    "channel".to_string(),
    str_field("Optional channel name for channel-style durable mail."),
  );
  props.insert(
    "queue_name".to_string(),
    str_field("Optional queue name for shared durable mailbox work items."),
  );
  props.insert(
    "priority".to_string(),
    str_field("Optional priority: Low, Normal, High, or Urgent."),
  );
  props.insert(
    "correlation_id".to_string(),
    str_field("Optional correlation id for message threading."),
  );
  props.insert(
    "task_id".to_string(),
    str_field("Optional task id that this message is about."),
  );
  collaboration_tool(
    "send_team_message",
    "Send durable team mail through the shared mailbox.",
    obj(props, &["message"]),
  )
}

fn send_team_nudge_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "message".to_string(),
    str_field("Ephemeral nudge body to send."),
  );
  props.insert(
    "recipient_thread_id".to_string(),
    str_field("Optional teammate selector. Accepts thread id, nickname, or @nickname. Omit to nudge a channel or the team."),
  );
  props.insert(
    "channel".to_string(),
    str_field("Optional channel name for broadcast-style nudges."),
  );
  props.insert(
    "queue_name".to_string(),
    str_field("Optional queue name for ephemeral work nudges."),
  );
  props.insert(
    "priority".to_string(),
    str_field("Optional priority: Low, Normal, High, or Urgent."),
  );
  props.insert(
    "correlation_id".to_string(),
    str_field("Optional correlation id for message threading."),
  );
  props.insert(
    "task_id".to_string(),
    str_field("Optional task id that this nudge is about."),
  );
  props.insert(
    "expires_at".to_string(),
    int_field("Optional Unix timestamp when this nudge expires."),
  );
  collaboration_tool(
    "send_team_nudge",
    "Send an ephemeral real-time nudge through the mailbox kernel.",
    obj(props, &["message"]),
  )
}

fn ack_team_message_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "message_id".to_string(),
    str_field("Mailbox message id to acknowledge."),
  );
  collaboration_tool(
    "ack_team_message",
    "Acknowledge receipt of a team mailbox message that requires ack.",
    obj(props, &["message_id"]),
  )
}

fn watch_team_inbox_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "after_version".to_string(),
    int_field("Optional mailbox version to watch after. Defaults to 0."),
  );
  props.insert(
    "timeout_ms".to_string(),
    int_field("Optional wait timeout in milliseconds."),
  );
  props.insert(
    "unread_only".to_string(),
    bool_field("When true, only return unread messages."),
  );
  collaboration_tool(
    "watch_team_inbox",
    "Wait for mailbox changes after a known version and return the visible inbox state.",
    obj(props, &[]),
  )
  .with_supports_parallel(true)
  .with_mutates_state(false)
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
    "owner_thread_id".to_string(),
    str_field("Optional owner selector: thread id, nickname, or @nickname."),
  );
  props.insert(
    "assignee_thread_id".to_string(),
    str_field("Optional assignee selector: thread id, nickname, or @nickname."),
  );
  props.insert(
    "workflow_run_id".to_string(),
    str_field("Optional run id to link this task back to a resumable team activity."),
  );
  props.insert(
    "requested_scopes".to_string(),
    scope_request_schema("Optional ownership scopes requested by this task."),
  );
  props.insert(
    "blocking_reason".to_string(),
    str_field("Optional manual blocking reason to create the task in a blocked state."),
  );
  props.insert(
    "scope_policy_override".to_string(),
    bool_field("Override shared scope policy safeguards for this task."),
  );
  collaboration_tool(
    "create_team_task",
    "Create a shared team task node on the common task graph, optionally linked to a resumable team run.",
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
        "Optional new task status: Pending, InProgress, Review, Completed, Failed, or Canceled."
          .to_string(),
      ),
    },
  );
  props.insert(
    "owner_thread_id".to_string(),
    str_field("Optional new owner selector: thread id, nickname, or @nickname."),
  );
  props.insert(
    "clear_owner".to_string(),
    bool_field("When true, clears the current owner."),
  );
  props.insert(
    "assignee_thread_id".to_string(),
    str_field("Optional new assignee selector: thread id, nickname, or @nickname."),
  );
  props.insert(
    "reviewer_thread_id".to_string(),
    str_field("Optional reviewer selector: thread id, nickname, or @nickname."),
  );
  props.insert(
    "clear_reviewer".to_string(),
    bool_field("When true, clears the current reviewer."),
  );
  props.insert(
    "clear_assignee".to_string(),
    bool_field("When true, clears the current assignee."),
  );
  props.insert(
    "note".to_string(),
    str_field("Optional note to append to the task history."),
  );
  props.insert(
    "requested_scopes".to_string(),
    scope_request_schema("Optional replacement list of requested ownership scopes."),
  );
  props.insert(
    "granted_scopes".to_string(),
    scope_request_schema("Optional replacement list of granted ownership scopes."),
  );
  props.insert(
    "review_state".to_string(),
    str_field("Optional review state: NotRequested, Requested, Approved, or ChangesRequested."),
  );
  props.insert(
    "scope_policy_override".to_string(),
    bool_field("Override shared scope policy safeguards for this task."),
  );
  collaboration_tool(
    "update_team_task",
    "Update a shared team task node status, ownership, scopes, or notes.",
    obj(props, &["task_id"]),
  )
}

fn release_task_leases_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "task_id".to_string(),
    str_field("Task id whose ownership leases should be released."),
  );
  collaboration_tool(
    "release_task_leases",
    "Release all ownership leases currently granted to a team task.",
    obj(props, &["task_id"]),
  )
}

fn force_release_lease_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "lease_id".to_string(),
    str_field("Ownership lease id to force release. Only @main may do this."),
  );
  collaboration_tool(
    "force_release_lease",
    "Force release a specific ownership lease when @main needs to recover a stale lock.",
    obj(props, &["lease_id"]),
  )
}
