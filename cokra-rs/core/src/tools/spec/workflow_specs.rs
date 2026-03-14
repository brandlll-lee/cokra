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
  vec![claim_next_team_task_tool(), plan_tool()]
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
    "Claim the next available team workflow task assigned to you or unassigned.",
    obj(BTreeMap::new(), &[]),
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
