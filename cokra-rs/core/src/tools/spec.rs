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
    unified_exec_tool(),
    apply_patch_tool(),
    edit_file_tool(),
    read_file_tool(),
    write_file_tool(),
    list_dir_tool(),
    grep_files_tool(),
    glob_tool(),
    code_search_tool(),
    search_tool(),
    spawn_agent_tool(),
    send_input_tool(),
    wait_tool(),
    close_agent_tool(),
    assign_team_task_tool(),
    claim_team_task_tool(),
    claim_next_team_task_tool(),
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
    plan_tool(),
    request_user_input_tool(),
    view_image_tool(),
    web_fetch_tool(),
    web_search_tool(),
    save_memory_tool(),
    diagnostics_tool(),
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
  props.insert(
    "sandbox_permissions".to_string(),
    str_field(
      "Optional sandbox mode: use_default, with_additional_permissions, or require_escalated.",
    ),
  );
  props.insert(
    "justification".to_string(),
    str_field("Short approval justification when requesting escalated or additional permissions."),
  );
  props.insert(
    "prefix_rule".to_string(),
    JsonSchema::Array {
      items: Box::new(str_field("Command prefix segment.")),
      description: Some(
        "Optional reusable command prefix rule, for example [\"cargo\", \"test\"].".to_string(),
      ),
    },
  );
  props.insert(
    "additional_permissions".to_string(),
    permission_profile_schema(),
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

fn unified_exec_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "command".to_string(),
    JsonSchema::Array {
      items: Box::new(str_field("Program or argument segment.")),
      description: Some(
        "Full argv as an array. The first element must be the program.".to_string(),
      ),
    },
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
  props.insert(
    "sandbox_permissions".to_string(),
    str_field(
      "Optional sandbox mode: use_default, with_additional_permissions, or require_escalated.",
    ),
  );
  props.insert(
    "justification".to_string(),
    str_field("Short approval justification when requesting escalated or additional permissions."),
  );
  props.insert(
    "prefix_rule".to_string(),
    JsonSchema::Array {
      items: Box::new(str_field("Command prefix segment.")),
      description: Some(
        "Optional reusable command prefix rule, for example [\"cargo\", \"test\"].".to_string(),
      ),
    },
  );
  props.insert(
    "additional_permissions".to_string(),
    permission_profile_schema(),
  );
  ToolSpec::new(
    "unified_exec",
    "Runs a pre-tokenized local command and returns its output. Use this when the command must be passed as argv instead of a shell string.",
    obj(props, &["command"]),
    None,
    ToolHandlerType::Function,
    mutating_permissions(),
  )
}

fn permission_profile_schema() -> JsonSchema {
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

fn edit_file_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "file_path".to_string(),
    str_field("Absolute path to the file to edit."),
  );
  props.insert(
    "old_string".to_string(),
    str_field(
      "The text to replace. Must match exactly (including whitespace and indentation). \
       Use an empty string to create a new file.",
    ),
  );
  props.insert(
    "new_string".to_string(),
    str_field("The replacement text. Must be different from old_string."),
  );
  props.insert(
    "replace_all".to_string(),
    bool_field(
      "When true, replace all occurrences of old_string. Default false (single replacement).",
    ),
  );
  ToolSpec::new(
    "edit_file",
    "Make precise text replacements in an existing file. Finds old_string and replaces it with \
     new_string. Use this for targeted edits instead of rewriting entire files. Supports CRLF \
     normalisation and whitespace-aware error hints. Use empty old_string to create a new file.",
    obj(props, &["file_path", "old_string", "new_string"]),
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

fn glob_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "pattern".to_string(),
    str_field("The glob pattern to match files against (e.g. \"*.rs\", \"**/*.ts\")."),
  );
  props.insert(
    "path".to_string(),
    str_field(
      "Optional directory to search in. Defaults to the session working directory. \
       Can be absolute or relative to the working directory.",
    ),
  );
  ToolSpec::new(
    "glob",
    "Find files by glob pattern. Returns matching file paths sorted alphabetically. \
     Uses ripgrep for fast gitignore-aware file discovery. Results are capped at 100 files. \
     Use this instead of shell commands like find or ls for file discovery.",
    obj(props, &["pattern"]),
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
    "include".to_string(),
    str_field("Optional glob filter to limit which files are searched."),
  );
  props.insert(
    "path".to_string(),
    str_field("Directory or file path to search. Defaults to the working directory."),
  );
  props.insert(
    "limit".to_string(),
    int_field("The maximum number of matching file paths to return."),
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

fn code_search_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("query".to_string(), str_field("Search query text."));
  props.insert(
    "path".to_string(),
    str_field(
      "Optional directory or file path to search. Defaults to the session working directory.",
    ),
  );
  props.insert(
    "limit".to_string(),
    int_field("Maximum number of files to return (default 10)."),
  );
  props.insert(
    "max_matches_per_file".to_string(),
    int_field("Maximum snippet matches per file (default 8)."),
  );
  ToolSpec::new(
    "code_search",
    "Search the local workspace for code/text matching a query and return ranked file hits with line-numbered snippets.",
    obj(props, &["query"]),
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

fn spawn_agent_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "task".to_string(),
    str_field("Initial task text for the spawned agent."),
  );
  props.insert(
    "message".to_string(),
    str_field("Alias of `task` for Codex-style compatibility."),
  );
  props.insert(
    "nickname".to_string(),
    str_field("Optional human-readable teammate name shown in team UI."),
  );
  props.insert("role".to_string(), str_field("Agent role."));
  props.insert(
    "agent_type".to_string(),
    str_field("Alias of `role` for Codex-style compatibility."),
  );
  ToolSpec::new(
    "spawn_agent",
    "Spawn a sub-agent and immediately start it on an initial task.",
    obj(props, &[]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
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
  ToolSpec::new(
    "send_input",
    "Send another message to a running or completed spawned agent.",
    obj(props, &["agent_id", "message"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
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
  ToolSpec::new(
    "wait",
    "Wait for spawned agents to finish before continuing.",
    obj(props, &[]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn close_agent_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "agent_id".to_string(),
    str_field("Target spawned agent id."),
  );
  ToolSpec::new(
    "close_agent",
    "Close and clean up a spawned agent when it is no longer needed.",
    obj(props, &["agent_id"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn assign_team_task_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("task_id".to_string(), str_field("Task id to assign."));
  props.insert(
    "assignee_thread_id".to_string(),
    str_field("Thread id of the teammate who should own the task."),
  );
  props.insert("note".to_string(), str_field("Optional assignment note."));
  ToolSpec::new(
    "assign_team_task",
    "Assign a shared team task to a specific teammate.",
    obj(props, &["task_id", "assignee_thread_id"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn claim_team_task_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("task_id".to_string(), str_field("Task id to claim."));
  props.insert(
    "note".to_string(),
    str_field("Optional claim note to append to the task history."),
  );
  ToolSpec::new(
    "claim_team_task",
    "Claim a shared team task for the current teammate and mark it in progress.",
    obj(props, &["task_id"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
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
  ToolSpec::new(
    "claim_team_messages",
    "Claim work items from a shared team mailbox queue.",
    obj(props, &["queue_name"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn claim_next_team_task_tool() -> ToolSpec {
  ToolSpec::new(
    "claim_next_team_task",
    "Claim the next available team workflow task assigned to you or unassigned.",
    obj(BTreeMap::new(), &[]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
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
  ToolSpec::new(
    "handoff_team_task",
    "Hand off a task to another teammate, optionally marking it ready for review.",
    obj(props, &["task_id", "to_thread_id"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn cleanup_team_tool() -> ToolSpec {
  ToolSpec::new(
    "cleanup_team",
    "Close all spawned agents and clear persisted team mailbox/task state for this workspace.",
    obj(BTreeMap::new(), &[]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
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
  ToolSpec::new(
    "submit_team_plan",
    "Submit a teammate work plan for approval before making mutating changes.",
    obj(props, &["summary", "steps"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
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
  ToolSpec::new(
    "approve_team_plan",
    "Approve or reject a teammate's submitted work plan.",
    obj(props, &["plan_id", "approved"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn team_status_tool() -> ToolSpec {
  ToolSpec::new(
    "team_status",
    "Return the shared team snapshot, including members, tasks, and unread mailbox counts.",
    obj(BTreeMap::new(), &[]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn send_team_message_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("message".to_string(), str_field("Message body to send."));
  props.insert(
    "recipient_thread_id".to_string(),
    str_field("Optional teammate thread id. Omit to broadcast to the whole team."),
  );
  ToolSpec::new(
    "send_team_message",
    "Send a direct or broadcast team mailbox message.",
    obj(props, &["message"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
}

fn read_team_messages_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "unread_only".to_string(),
    bool_field("When true, only return unread mailbox messages."),
  );
  ToolSpec::new(
    "read_team_messages",
    "Read your team mailbox messages and mark them as seen.",
    obj(props, &[]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
  )
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
  ToolSpec::new(
    "create_team_task",
    "Create a shared team task on the common task board.",
    obj(props, &["title"]),
    None,
    ToolHandlerType::Function,
    default_permissions(),
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
  ToolSpec::new(
    "update_team_task",
    "Update a shared team task status, assignee, or notes.",
    obj(props, &["task_id"]),
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
  option_props.insert(
    "label".to_string(),
    str_field("Short label for this option."),
  );
  option_props.insert(
    "description".to_string(),
    str_field("Single-sentence description of the effect of choosing this option."),
  );

  let mut question_props = BTreeMap::new();
  question_props.insert(
    "id".to_string(),
    str_field("Stable identifier for the question."),
  );
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
      items: Box::new(obj(
        question_props,
        &["id", "header", "question", "options"],
      )),
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

fn web_fetch_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "url".to_string(),
    str_field("The URL to fetch content from. Must start with http:// or https://."),
  );
  props.insert(
    "format".to_string(),
    str_field(
      "Response format: 'text' (default, HTML converted to plain text), \
       'html' (raw HTML), or 'raw' (unprocessed response body).",
    ),
  );
  props.insert(
    "timeout".to_string(),
    int_field("Optional timeout in seconds (default 30, max 120)."),
  );
  ToolSpec::new(
    "web_fetch",
    "Fetch content from a URL. Supports HTML-to-text conversion for readable output. \
     Use this to read web documentation, API references, or any online resource. \
     Response is capped at 5MB with automatic text truncation for large pages.",
    obj(props, &["url"]),
    None,
    ToolHandlerType::Function,
    ToolPermissions {
      requires_approval: true,
      allow_network: true,
      allow_fs_write: false,
    },
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

fn web_search_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "query".to_string(),
    str_field("The search query to look up on the web."),
  );
  props.insert(
    "num_results".to_string(),
    int_field("Number of results to return (default: 8, max: 20)."),
  );
  props.insert(
    "livecrawl".to_string(),
    str_field(
      "Live crawl mode for Exa backend: 'fallback' (default) or 'preferred'.",
    ),
  );
  props.insert(
    "context_max_characters".to_string(),
    int_field("Maximum characters for context string (Exa backend, default: 10000)."),
  );
  ToolSpec::new(
    "web_search",
    "Search the web for information. Supports multiple backends: \
     Exa (default, no key needed), Brave (set BRAVE_SEARCH_API_KEY), \
     or SearXNG self-hosted (set SEARXNG_BASE_URL). \
     Returns search results with titles, URLs, and summaries.",
    obj(props, &["query"]),
    None,
    ToolHandlerType::Function,
    ToolPermissions {
      requires_approval: true,
      allow_network: true,
      allow_fs_write: false,
    },
  )
}

fn save_memory_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "fact".to_string(),
    str_field(
      "The specific fact or piece of information to remember. \
       Should be a clear, self-contained statement.",
    ),
  );
  ToolSpec::new(
    "save_memory",
    "Save a piece of information to persistent memory (~/.cokra/memory.md). \
     Use this to remember important facts, preferences, or context about the user \
     or project that should persist across sessions.",
    obj(props, &["fact"]),
    None,
    ToolHandlerType::Function,
    ToolPermissions {
      requires_approval: true,
      allow_network: false,
      allow_fs_write: true,
    },
  )
}

fn diagnostics_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "path".to_string(),
    str_field(
      "Absolute or relative path to the source file to get diagnostics for.",
    ),
  );
  props.insert(
    "max_diagnostics".to_string(),
    int_field("Maximum number of diagnostics to return (default: 50)."),
  );
  ToolSpec::new(
    "diagnostics",
    "Get LSP diagnostics (errors, warnings, hints) for a source file. \
     Spawns the appropriate language server (rust-analyzer, typescript-language-server, \
     pylsp, gopls, clangd, etc.) and returns all diagnostics. \
     The language server must be installed and on PATH.",
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
