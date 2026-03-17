use std::collections::BTreeMap;

use super::JsonSchema;
use super::ToolHandlerType;
use super::ToolPermissions;
use super::ToolSourceKind;
use super::ToolSpec;
use super::default_permissions;
use super::obj;
use super::str_field;

pub(crate) fn build_specs() -> Vec<ToolSpec> {
  vec![
    add_task_dependency_tool(),
    remove_task_dependency_tool(),
    block_task_tool(),
    unblock_task_tool(),
    list_ready_tasks_tool(),
    claim_ready_task_tool(),
    claim_next_team_task_tool(),
    plan_tool(),
  ]
}

fn workflow_tool(name: &str, description: impl Into<String>, input_schema: JsonSchema) -> ToolSpec {
  workflow_tool_with_permissions(name, description, input_schema, default_permissions())
}

fn workflow_tool_with_permissions(
  name: &str,
  description: impl Into<String>,
  input_schema: JsonSchema,
  permissions: ToolPermissions,
) -> ToolSpec {
  ToolSpec::new(
    name,
    description,
    input_schema,
    None,
    ToolHandlerType::Function,
    permissions,
  )
  .with_source_kind(ToolSourceKind::BuiltinWorkflow)
  .with_permission_key("workflow")
  .with_supports_parallel(false)
  .with_mutates_state(true)
}

fn claim_next_team_task_tool() -> ToolSpec {
  workflow_tool(
    "claim_next_team_task",
    "Compatibility wrapper that claims the next ready team task assigned to you or unassigned.",
    obj(BTreeMap::new(), &[]),
  )
}

fn add_task_dependency_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "task_id".to_string(),
    str_field("Task id to be blocked by another task."),
  );
  props.insert(
    "dependency_task_id".to_string(),
    str_field("Blocking dependency task id."),
  );
  props.insert(
    "reason".to_string(),
    str_field("Optional reason describing the dependency edge."),
  );
  workflow_tool(
    "add_task_dependency",
    "Add a dependency edge so one team task is blocked by another.",
    obj(props, &["task_id", "dependency_task_id"]),
  )
}

fn remove_task_dependency_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("task_id".to_string(), str_field("Dependent task id."));
  props.insert(
    "dependency_task_id".to_string(),
    str_field("Blocking dependency task id to remove."),
  );
  workflow_tool(
    "remove_task_dependency",
    "Remove an existing dependency edge from the team task graph.",
    obj(props, &["task_id", "dependency_task_id"]),
  )
}

fn block_task_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "task_id".to_string(),
    str_field("Task id to manually block."),
  );
  props.insert("reason".to_string(), str_field("Manual blocking reason."));
  workflow_tool(
    "block_task",
    "Add a manual blocker to a team task node.",
    obj(props, &["task_id", "reason"]),
  )
}

fn unblock_task_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("task_id".to_string(), str_field("Task id to unblock."));
  props.insert(
    "blocker_id".to_string(),
    str_field("Optional specific manual blocker id to clear. Omit to clear all manual blockers."),
  );
  workflow_tool(
    "unblock_task",
    "Clear one or all manual blockers from a team task node.",
    obj(props, &["task_id"]),
  )
}

fn list_ready_tasks_tool() -> ToolSpec {
  workflow_tool(
    "list_ready_tasks",
    "List team task nodes that are currently ready to be claimed by the caller.",
    obj(BTreeMap::new(), &[]),
  )
  .with_mutates_state(false)
  .with_supports_parallel(true)
}

fn claim_ready_task_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("task_id".to_string(), str_field("Ready task id to claim."));
  props.insert(
    "note".to_string(),
    str_field("Optional note to append to the task history when claiming."),
  );
  workflow_tool(
    "claim_ready_task",
    "Claim a ready task node, transition it to in-progress, and grant its requested scopes.",
    obj(props, &["task_id"]),
  )
}

fn plan_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("text".to_string(), str_field("Plan text."));
  workflow_tool(
    "plan",
    "Emit a plan item and persist it into workflow state.",
    obj(props, &["text"]),
  )
  .with_permission_key("plan")
}
