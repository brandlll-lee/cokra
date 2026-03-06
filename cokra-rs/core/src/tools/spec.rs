use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;

/// Whether additional properties are allowed, and if so, any required schema.
///
/// Mirrors codex-rs `AdditionalProperties` — used in tool parameter schemas
/// to tell the model that no extra keys are allowed (`false`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum AdditionalProperties {
  Boolean(bool),
  Schema(Box<JsonSchema>),
}

impl From<bool> for AdditionalProperties {
  fn from(b: bool) -> Self {
    Self::Boolean(b)
  }
}

impl From<JsonSchema> for AdditionalProperties {
  fn from(s: JsonSchema) -> Self {
    Self::Schema(Box::new(s))
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
    required: Some(required.iter().map(|s| s.to_string()).collect()),
    additional_properties: Some(false.into()),
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
  props.insert(
    "command".to_string(),
    str_field(
      "The command to execute. On Linux/macOS most commands should be prefixed with bash -lc.",
    ),
  );
  props.insert(
    "timeout_ms".to_string(),
    int_field("The timeout for the command in milliseconds."),
  );
  props.insert(
    "workdir".to_string(),
    str_field(
      "The working directory to execute the command in. Always set this instead of using cd.",
    ),
  );
  ToolSpec::new(
    "shell",
    "Runs a shell command and returns its output. Use the dedicated read_file, list_dir, and grep_files tools instead of shell commands like cat, ls, find, or grep for reading files and exploring the filesystem.",
    obj(props, &["command"]),
    None,
    ToolHandlerType::Function,
    mutating_permissions(),
  )
}

fn apply_patch_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "patch".to_string(),
    str_field(concat!(
      "The patch to apply. Use a stripped-down diff format:\n",
      "*** Begin Patch\n",
      "*** Update File: path/to/file\n",
      "@@ context_line\n",
      "- removed_line\n",
      "+ added_line\n",
      "*** End Patch\n",
      "Also supports *** Add File and *** Delete File headers."
    )),
  );
  ToolSpec::new(
    "apply_patch",
    "Apply a patch to create, update, or delete files. Use this tool for all file edits. Do NOT use shell commands like cat with heredoc or echo redirection to write files.",
    obj(props, &["patch"]),
    None,
    ToolHandlerType::Function,
    mutating_permissions(),
  )
}

fn read_file_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "file_path".to_string(),
    str_field("Absolute path to the file to read."),
  );
  props.insert(
    "offset".to_string(),
    int_field("The 1-indexed line number to start reading from. Defaults to 1."),
  );
  props.insert(
    "limit".to_string(),
    int_field("The maximum number of lines to return. Defaults to 2000."),
  );
  ToolSpec::new(
    "read_file",
    "Reads a local file with 1-indexed line numbers. Use this instead of shell commands like cat, head, or tail.",
    obj(props, &["file_path"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn write_file_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "file_path".to_string(),
    str_field("Absolute path to the file to write."),
  );
  props.insert(
    "content".to_string(),
    str_field("The full content to write to the file."),
  );
  ToolSpec::new(
    "write_file",
    "Write content to a file, creating it if it does not exist. Parent directories are created automatically. Prefer apply_patch for editing existing files.",
    obj(props, &["file_path", "content"]),
    None,
    ToolHandlerType::Function,
    mutating_permissions(),
  )
}

fn list_dir_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "dir_path".to_string(),
    str_field("Absolute path to the directory to list."),
  );
  props.insert(
    "offset".to_string(),
    int_field("The entry number to start listing from. Must be 1 or greater."),
  );
  props.insert(
    "limit".to_string(),
    int_field("The maximum number of entries to return."),
  );
  props.insert(
    "depth".to_string(),
    int_field("The maximum directory depth to traverse. Must be 1 or greater."),
  );
  ToolSpec::new(
    "list_dir",
    "Lists entries in a local directory with 1-indexed entry numbers and simple type labels. Use this instead of shell commands like ls or find.",
    obj(props, &["dir_path"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn grep_files_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "pattern".to_string(),
    str_field("The pattern to search for in file contents."),
  );
  props.insert(
    "path".to_string(),
    str_field("Directory or file path to search. Defaults to the working directory."),
  );
  ToolSpec::new(
    "grep_files",
    "Finds files whose contents match the pattern and lists them by modification time. Use this instead of shell commands like grep or rg for searching code.",
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
      required: Some(Vec::new()),
      additional_properties: None,
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
  let mut option_props = BTreeMap::new();
  option_props.insert("label".to_string(), str_field("Short label for this option."));
  option_props.insert(
    "description".to_string(),
    str_field("Single-sentence description of the effect of choosing this option."),
  );

  let mut question_props = BTreeMap::new();
  question_props.insert("id".to_string(), str_field("Stable identifier for the question."));
  question_props.insert(
    "header".to_string(),
    str_field("Short header label shown for the question."),
  );
  question_props.insert(
    "question".to_string(),
    str_field("Single-sentence prompt shown to the user."),
  );
  question_props.insert(
    "options".to_string(),
    JsonSchema::Array {
      items: Box::new(obj(option_props, &["label", "description"])),
      description: Some("Two or three mutually exclusive options for the question.".to_string()),
    },
  );

  let mut props = BTreeMap::new();
  props.insert(
    "questions".to_string(),
    JsonSchema::Array {
      items: Box::new(obj(question_props, &["id", "header", "question", "options"])),
      description: Some("One to three short questions to ask the user.".to_string()),
    },
  );
  ToolSpec::new(
    "request_user_input",
    "Request user input for one to three short questions and wait for the response.",
    obj(props, &["questions"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn view_image_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "path".to_string(),
    str_field("Local filesystem path to an image file."),
  );
  ToolSpec::new(
    "view_image",
    "View a local image from the filesystem. Only use if given a full filepath by the user.",
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
}
