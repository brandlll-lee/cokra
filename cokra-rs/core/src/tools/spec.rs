use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// JSON schema representation for tool input/output contracts.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    Self {
      name: name.into(),
      description: description.into(),
      input_schema,
      output_schema,
      handler_type,
      permissions,
    }
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
  vec![
    shell_tool(),
    apply_patch_tool(),
    read_file_tool(),
    write_file_tool(),
    list_dir_tool(),
    grep_files_tool(),
    search_tool(),
    mcp_tool(),
    spawn_agent_tool(),
    plan_tool(),
    request_user_input_tool(),
    view_image_tool(),
  ]
}

fn obj(properties: BTreeMap<String, JsonSchema>, required: &[&str]) -> JsonSchema {
  JsonSchema::Object {
    properties,
    required: if required.is_empty() {
      None
    } else {
      Some(required.iter().map(|s| s.to_string()).collect())
    },
  }
}

fn str_field(desc: &str) -> JsonSchema {
  JsonSchema::String {
    description: Some(desc.to_string()),
  }
}

fn int_field(desc: &str) -> JsonSchema {
  JsonSchema::Number {
    description: Some(desc.to_string()),
  }
}

fn bool_field(desc: &str) -> JsonSchema {
  JsonSchema::Boolean {
    description: Some(desc.to_string()),
  }
}

fn default_permissions() -> ToolPermissions {
  ToolPermissions::default()
}

fn mutating_permissions() -> ToolPermissions {
  ToolPermissions {
    requires_approval: true,
    allow_network: false,
    allow_fs_write: true,
  }
}

fn shell_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("command".to_string(), str_field("Shell command"));
  props.insert("timeout_ms".to_string(), int_field("Timeout milliseconds"));
  props.insert("workdir".to_string(), str_field("Working directory"));
  ToolSpec::new(
    "shell",
    "Execute a shell command",
    obj(props, &["command"]),
    None,
    ToolHandlerType::Function,
    mutating_permissions(),
  )
}

fn apply_patch_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("patch".to_string(), str_field("Unified patch text"));
  ToolSpec::new(
    "apply_patch",
    "Apply patch to files",
    obj(props, &["patch"]),
    None,
    ToolHandlerType::Function,
    mutating_permissions(),
  )
}

fn read_file_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("file_path".to_string(), str_field("File path"));
  props.insert("offset".to_string(), int_field("Start line offset"));
  props.insert("limit".to_string(), int_field("Maximum lines"));
  ToolSpec::new(
    "read_file",
    "Read text file content",
    obj(props, &["file_path"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn write_file_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("file_path".to_string(), str_field("File path"));
  props.insert("content".to_string(), str_field("File content"));
  ToolSpec::new(
    "write_file",
    "Write content to file",
    obj(props, &["file_path", "content"]),
    None,
    ToolHandlerType::Function,
    mutating_permissions(),
  )
}

fn list_dir_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("dir_path".to_string(), str_field("Directory path"));
  props.insert("recursive".to_string(), bool_field("Recursive listing"));
  ToolSpec::new(
    "list_dir",
    "List directory entries",
    obj(props, &["dir_path"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn grep_files_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("pattern".to_string(), str_field("Regex pattern"));
  props.insert("path".to_string(), str_field("Search root path"));
  ToolSpec::new(
    "grep_files",
    "Search files by pattern",
    obj(props, &["pattern"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn search_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("query".to_string(), str_field("Search query"));
  ToolSpec::new(
    "search_tool",
    "Search available tools",
    obj(props, &["query"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn mcp_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("server".to_string(), str_field("MCP server name"));
  props.insert("tool".to_string(), str_field("MCP tool name"));
  props.insert(
    "arguments".to_string(),
    JsonSchema::Object {
      properties: BTreeMap::new(),
      required: None,
    },
  );
  ToolSpec::new(
    "mcp",
    "Invoke an MCP tool",
    obj(props, &["server", "tool"]),
    None,
    ToolHandlerType::Mcp,
    default_permissions(),
  )
}

fn spawn_agent_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("task".to_string(), str_field("Task text"));
  props.insert("role".to_string(), str_field("Agent role"));
  ToolSpec::new(
    "spawn_agent",
    "Spawn sub-agent",
    obj(props, &["task"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn plan_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("text".to_string(), str_field("Plan text"));
  ToolSpec::new(
    "plan",
    "Emit a plan item",
    obj(props, &["text"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn request_user_input_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("prompt".to_string(), str_field("Prompt to user"));
  ToolSpec::new(
    "request_user_input",
    "Request user input",
    obj(props, &["prompt"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn view_image_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("path".to_string(), str_field("Image path"));
  ToolSpec::new(
    "view_image",
    "View image from local filesystem",
    obj(props, &["path"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn object_schema_omits_null_required_field() {
    let schema = JsonSchema::Object {
      properties: BTreeMap::new(),
      required: None,
    };
    let value = schema.to_value();

    assert_eq!(value["type"], "object");
    assert!(value.get("required").is_none());
  }
}
