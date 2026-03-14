use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;

#[path = "spec/collaboration_specs.rs"]
mod collaboration_specs;
#[path = "spec/primitive_specs.rs"]
mod primitive_specs;
#[path = "spec/workflow_specs.rs"]
mod workflow_specs;

pub use primitive_specs::skill_tool_with_description;

/// Whether additional properties are allowed, and if so, any required schema.
///
/// Mirrors codex-rs `AdditionalProperties`, which is used to tell the model
/// when no extra keys are allowed in JSON-schema arguments.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum AdditionalProperties {
  Boolean(bool),
  Schema(Box<JsonSchema>),
}

impl From<bool> for AdditionalProperties {
  fn from(value: bool) -> Self {
    Self::Boolean(value)
  }
}

impl From<JsonSchema> for AdditionalProperties {
  fn from(value: JsonSchema) -> Self {
    Self::Schema(Box::new(value))
  }
}

/// JSON schema representation for tool input/output contracts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum JsonSchema {
  String {
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
  },
  Number {
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
  },
  Boolean {
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
  },
  Array {
    items: Box<JsonSchema>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
  },
  Object {
    properties: BTreeMap<String, JsonSchema>,
    #[serde(skip_serializing_if = "Option::is_none")]
    required: Option<Vec<String>>,
    #[serde(
      rename = "additionalProperties",
      skip_serializing_if = "Option::is_none"
    )]
    additional_properties: Option<AdditionalProperties>,
  },
}

impl JsonSchema {
  pub fn to_value(&self) -> serde_json::Value {
    serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({ "type": "object" }))
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ToolHandlerType {
  Function,
  Mcp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolSourceKind {
  #[default]
  BuiltinPrimitive,
  BuiltinCollaboration,
  BuiltinWorkflow,
  Cli,
  Api,
  Mcp,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolPermissions {
  pub requires_approval: bool,
  pub allow_network: bool,
  pub allow_fs_write: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
  pub name: String,
  pub description: String,
  pub input_schema: JsonSchema,
  pub output_schema: Option<JsonSchema>,
  pub handler_type: ToolHandlerType,
  pub permissions: ToolPermissions,
  #[serde(default)]
  pub source_kind: ToolSourceKind,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub permission_key: Option<String>,
  #[serde(default = "tool_supports_parallel_default")]
  pub supports_parallel: bool,
  #[serde(default)]
  pub mutates_state: bool,
}

impl ToolSpec {
  pub fn new(
    name: impl Into<String>,
    description: impl Into<String>,
    input_schema: JsonSchema,
    output_schema: Option<JsonSchema>,
    handler_type: ToolHandlerType,
    permissions: ToolPermissions,
  ) -> Self {
    let name = name.into();
    let source_kind = match handler_type {
      ToolHandlerType::Function => ToolSourceKind::BuiltinPrimitive,
      ToolHandlerType::Mcp => ToolSourceKind::Mcp,
    };
    let mutates_state = permissions.allow_fs_write;
    Self {
      permission_key: Some(name.clone()),
      name,
      description: description.into(),
      input_schema,
      output_schema,
      handler_type,
      permissions,
      source_kind,
      supports_parallel: !mutates_state,
      mutates_state,
    }
  }

  pub fn with_source_kind(mut self, source_kind: ToolSourceKind) -> Self {
    self.source_kind = source_kind;
    self
  }

  pub fn with_permission_key(mut self, permission_key: impl Into<String>) -> Self {
    self.permission_key = Some(permission_key.into());
    self
  }

  pub fn with_supports_parallel(mut self, supports_parallel: bool) -> Self {
    self.supports_parallel = supports_parallel;
    self
  }

  pub fn with_mutates_state(mut self, mutates_state: bool) -> Self {
    self.mutates_state = mutates_state;
    self
  }

  pub fn to_model_tool(&self) -> crate::model::Tool {
    crate::model::Tool::function(crate::model::FunctionDefinition {
      name: self.name.clone(),
      description: self.description.clone(),
      parameters: self.input_schema.to_value(),
    })
  }
}

pub fn build_specs() -> Vec<ToolSpec> {
  let mut by_name = primitive_specs::build_specs()
    .into_iter()
    .chain(collaboration_specs::build_specs())
    .chain(workflow_specs::build_specs())
    .map(|spec| (spec.name.clone(), spec))
    .collect::<std::collections::HashMap<_, _>>();

  const ORDER: &[&str] = &[
    "shell",
    "unified_exec",
    "apply_patch",
    "edit_file",
    "read_file",
    "write_file",
    "list_dir",
    "grep_files",
    "glob",
    "code_search",
    "search_tool",
    "inspect_tool",
    "active_tool_status",
    "activate_tools",
    "deactivate_tools",
    "reset_active_tools",
    "integration_status",
    "connect_integration",
    "install_integration",
    "tool_audit_log",
    "list_mcp_resources",
    "list_mcp_resource_templates",
    "read_mcp_resource",
    "spawn_agent",
    "send_input",
    "wait",
    "close_agent",
    "assign_team_task",
    "claim_team_task",
    "claim_next_team_task",
    "claim_team_messages",
    "handoff_team_task",
    "cleanup_team",
    "submit_team_plan",
    "approve_team_plan",
    "team_status",
    "send_team_message",
    "read_team_messages",
    "create_team_task",
    "update_team_task",
    "plan",
    "request_user_input",
    "view_image",
    "web_fetch",
    "web_search",
    "save_memory",
    "diagnostics",
    "skill",
    "read_many_files",
    "todo_write",
  ];

  let mut ordered = ORDER
    .iter()
    .filter_map(|name| by_name.remove(*name))
    .collect::<Vec<_>>();
  let mut remaining = by_name.into_values().collect::<Vec<_>>();
  remaining.sort_by(|left, right| left.name.cmp(&right.name));
  ordered.extend(remaining);
  ordered
}

pub(crate) fn obj(properties: BTreeMap<String, JsonSchema>, required: &[&str]) -> JsonSchema {
  JsonSchema::Object {
    properties,
    required: Some(required.iter().map(|value| value.to_string()).collect()),
    additional_properties: Some(false.into()),
  }
}

pub(crate) fn str_field(description: &str) -> JsonSchema {
  JsonSchema::String {
    description: Some(description.to_string()),
  }
}

pub(crate) fn int_field(description: &str) -> JsonSchema {
  JsonSchema::Number {
    description: Some(description.to_string()),
  }
}

pub(crate) fn bool_field(description: &str) -> JsonSchema {
  JsonSchema::Boolean {
    description: Some(description.to_string()),
  }
}

pub(crate) fn default_permissions() -> ToolPermissions {
  ToolPermissions::default()
}

pub(crate) fn mutating_permissions() -> ToolPermissions {
  ToolPermissions {
    requires_approval: true,
    allow_network: false,
    allow_fs_write: true,
  }
}

pub(crate) fn permission_profile_schema() -> JsonSchema {
  let mut network_props = BTreeMap::new();
  network_props.insert(
    "enabled".to_string(),
    bool_field("Whether network access is requested."),
  );

  let mut fs_props = BTreeMap::new();
  fs_props.insert(
    "read".to_string(),
    JsonSchema::Array {
      items: Box::new(str_field("Readable path.")),
      description: Some("Additional readable filesystem paths.".to_string()),
    },
  );
  fs_props.insert(
    "write".to_string(),
    JsonSchema::Array {
      items: Box::new(str_field("Writable path.")),
      description: Some("Additional writable filesystem paths.".to_string()),
    },
  );

  let mut props = BTreeMap::new();
  props.insert(
    "network".to_string(),
    JsonSchema::Object {
      properties: network_props,
      required: Some(Vec::new()),
      additional_properties: Some(false.into()),
    },
  );
  props.insert(
    "file_system".to_string(),
    JsonSchema::Object {
      properties: fs_props,
      required: Some(Vec::new()),
      additional_properties: Some(false.into()),
    },
  );
  props.insert(
    "macos".to_string(),
    JsonSchema::Object {
      properties: BTreeMap::new(),
      required: Some(Vec::new()),
      additional_properties: None,
    },
  );

  JsonSchema::Object {
    properties: props,
    required: Some(Vec::new()),
    additional_properties: Some(false.into()),
  }
}

fn tool_supports_parallel_default() -> bool {
  true
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn object_schema_always_serializes_required_as_array() {
    let schema = JsonSchema::Object {
      properties: BTreeMap::new(),
      required: Some(Vec::new()),
      additional_properties: None,
    };
    let value = schema.to_value();

    assert_eq!(value["type"], "object");
    assert_eq!(value["required"], serde_json::json!([]));
  }

  #[test]
  fn object_schema_serializes_additional_properties_false() {
    let schema = JsonSchema::Object {
      properties: BTreeMap::new(),
      required: Some(Vec::new()),
      additional_properties: Some(false.into()),
    };
    let value = schema.to_value();

    assert_eq!(value["type"], "object");
    assert_eq!(value["additionalProperties"], serde_json::json!(false));
  }

  #[test]
  fn tool_spec_defaults_metadata_from_handler_and_permissions() {
    let spec = ToolSpec::new(
      "shell",
      "Run a command",
      obj(BTreeMap::new(), &[]),
      None,
      ToolHandlerType::Function,
      mutating_permissions(),
    );

    assert_eq!(spec.source_kind, ToolSourceKind::BuiltinPrimitive);
    assert_eq!(spec.permission_key.as_deref(), Some("shell"));
    assert!(!spec.supports_parallel);
    assert!(spec.mutates_state);
  }
}
