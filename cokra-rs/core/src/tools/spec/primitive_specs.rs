use std::collections::BTreeMap;

use super::JsonSchema;
use super::ToolHandlerType;
use super::ToolPermissions;
use super::ToolSourceKind;
use super::ToolSpec;
use super::bool_field;
use super::default_permissions;
use super::int_field;
use super::mutating_permissions;
use super::obj;
use super::permission_profile_schema;
use super::str_field;

pub(crate) fn build_specs() -> Vec<ToolSpec> {
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
    inspect_tool(),
    active_tool_status_tool(),
    activate_tools_tool(),
    deactivate_tools_tool(),
    reset_active_tools_tool(),
    integration_status_tool(),
    connect_integration_tool(),
    install_integration_tool(),
    tool_audit_log_tool(),
    list_mcp_resources_tool(),
    list_mcp_resource_templates_tool(),
    read_mcp_resource_tool(),
    request_user_input_tool(),
    view_image_tool(),
    web_fetch_tool(),
    web_search_tool(),
    save_memory_tool(),
    diagnostics_tool(),
    skill_tool(),
    read_many_files_tool(),
    todo_write_tool(),
  ]
}

fn primitive_tool(
  name: &str,
  description: impl Into<String>,
  input_schema: JsonSchema,
  permissions: ToolPermissions,
) -> ToolSpec {
  let mutates_state = permissions.allow_fs_write;
  ToolSpec::new(
    name,
    description,
    input_schema,
    None,
    ToolHandlerType::Function,
    permissions,
  )
  .with_source_kind(ToolSourceKind::BuiltinPrimitive)
  .with_supports_parallel(!mutates_state)
  .with_mutates_state(mutates_state)
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
  primitive_tool(
    "shell",
    "Runs a shell command and returns its output. Use the dedicated read_file, list_dir, and grep_files tools instead of shell commands like cat, ls, find, or grep for reading files and exploring the filesystem.",
    obj(props, &["command"]),
    mutating_permissions(),
  )
  .with_permission_key("exec")
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
  primitive_tool(
    "unified_exec",
    "Runs a pre-tokenized local command and returns its output. Use this when the command must be passed as argv instead of a shell string.",
    obj(props, &["command"]),
    mutating_permissions(),
  )
  .with_permission_key("exec")
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
      "The text to replace. Must match exactly, including whitespace and indentation. Use an empty string to create a new file.",
    ),
  );
  props.insert(
    "new_string".to_string(),
    str_field("The replacement text. Must be different from old_string."),
  );
  props.insert(
    "replace_all".to_string(),
    bool_field(
      "When true, replace all occurrences of old_string. Default false for a single replacement.",
    ),
  );
  primitive_tool(
    "edit_file",
    "Make precise text replacements in an existing file. Use this for targeted edits instead of rewriting entire files.",
    obj(props, &["file_path", "old_string", "new_string"]),
    mutating_permissions(),
  )
  .with_permission_key("edit")
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
  primitive_tool(
    "apply_patch",
    "Apply a patch to create, update, or delete files. Use this tool for all file edits. Do not use shell commands or redirection to write files.",
    obj(props, &["patch"]),
    mutating_permissions(),
  )
  .with_permission_key("edit")
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
  primitive_tool(
    "read_file",
    "Reads a local file with 1-indexed line numbers. Use this instead of shell commands like cat, head, or tail.",
    obj(props, &["file_path"]),
    default_permissions(),
  )
  .with_permission_key("read")
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
  primitive_tool(
    "write_file",
    "Write content to a file, creating it if it does not exist. Prefer apply_patch for editing existing files.",
    obj(props, &["file_path", "content"]),
    mutating_permissions(),
  )
  .with_permission_key("edit")
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
  primitive_tool(
    "list_dir",
    "Lists entries in a local directory with 1-indexed entry numbers and simple type labels.",
    obj(props, &["dir_path"]),
    default_permissions(),
  )
  .with_permission_key("read")
}

fn glob_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "pattern".to_string(),
    str_field("The glob pattern to match files against, for example *.rs or **/*.ts."),
  );
  props.insert(
    "path".to_string(),
    str_field(
      "Optional directory to search in. Defaults to the session working directory. Can be absolute or relative to the working directory.",
    ),
  );
  primitive_tool(
    "glob",
    "Find files by glob pattern. Returns matching file paths sorted alphabetically and uses ripgrep for fast gitignore-aware discovery.",
    obj(props, &["pattern"]),
    default_permissions(),
  )
  .with_permission_key("read")
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
  primitive_tool(
    "grep_files",
    "Find files whose contents match the pattern and list them by modification time.",
    obj(props, &["pattern"]),
    default_permissions(),
  )
  .with_permission_key("read")
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
    int_field("Maximum number of files to return. Default 10."),
  );
  props.insert(
    "max_matches_per_file".to_string(),
    int_field("Maximum snippet matches per file. Default 8."),
  );
  primitive_tool(
    "code_search",
    "Search the local workspace for code or text matching a query and return ranked file hits with line-numbered snippets.",
    obj(props, &["query"]),
    default_permissions(),
  )
  .with_permission_key("read")
}

fn search_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert("query".to_string(), str_field("Search query."));
  props.insert(
    "limit".to_string(),
    int_field("Maximum number of capabilities to return. Default 8, max 20."),
  );
  primitive_tool(
    "search_tool",
    "Search the current runtime tool space by tool name, alias, description, source metadata, and input schema keys.",
    obj(props, &["query"]),
    default_permissions(),
  )
  .with_permission_key("tool_catalog")
}

fn inspect_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "name".to_string(),
    str_field("Capability name or alias to inspect."),
  );
  primitive_tool(
    "inspect_tool",
    "Inspect a runtime tool definition, including aliases, permissions, input keys, source metadata, and resource locators.",
    obj(props, &["name"]),
    default_permissions(),
  )
  .with_permission_key("tool_catalog")
}

fn active_tool_status_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "limit".to_string(),
    int_field("Maximum number of active/inactive external tool names to include."),
  );
  primitive_tool(
    "active_tool_status",
    "Summarize the current runtime tool space, including active and inactive external tools by source.",
    obj(props, &[]),
    default_permissions(),
  )
  .with_permission_key("read")
}

fn activate_tools_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "names".to_string(),
    JsonSchema::Array {
      items: Box::new(str_field("Tool name, id, or alias to activate.")),
      description: Some("External tool names to activate for the current runtime.".to_string()),
    },
  );
  primitive_tool(
    "activate_tools",
    "Activate external tools in the current runtime so the model can call them directly in subsequent steps.",
    obj(props, &["names"]),
    default_permissions(),
  )
  .with_permission_key("tool_space")
}

fn deactivate_tools_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "names".to_string(),
    JsonSchema::Array {
      items: Box::new(str_field("Tool name, id, or alias to deactivate.")),
      description: Some("External tool names to hide from the active runtime surface.".to_string()),
    },
  );
  primitive_tool(
    "deactivate_tools",
    "Deactivate external tools in the current runtime without removing their integrations.",
    obj(props, &["names"]),
    default_permissions(),
  )
  .with_permission_key("tool_space")
}

fn reset_active_tools_tool() -> ToolSpec {
  primitive_tool(
    "reset_active_tools",
    "Reset external tool activation so all discovered integrations return to their default active state.",
    obj(BTreeMap::new(), &[]),
    default_permissions(),
  )
  .with_permission_key("tool_space")
}

fn integration_status_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "name".to_string(),
    str_field("Optional integration name to inspect. Omit to list all discovered integrations."),
  );
  primitive_tool(
    "integration_status",
    "List discovered MCP, CLI, and API integrations with bootstrap state, declared tools, and active tool coverage.",
    obj(props, &[]),
    default_permissions(),
  )
  .with_permission_key("read")
}

fn connect_integration_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "name".to_string(),
    str_field("Integration name to connect and activate in the current runtime."),
  );
  primitive_tool(
    "connect_integration",
    "Validate an integration's prerequisites and activate its tools in the current runtime.",
    obj(props, &["name"]),
    default_permissions(),
  )
  .with_permission_key("tool_space")
}

fn install_integration_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "name".to_string(),
    str_field("Integration name whose declared install command should be executed."),
  );
  primitive_tool(
    "install_integration",
    "Run the declared install/bootstrap command for an integration and report the resulting readiness.",
    obj(props, &["name"]),
    mutating_permissions(),
  )
  .with_permission_key("exec")
}

fn tool_audit_log_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "limit".to_string(),
    int_field("Maximum number of recent tool calls to return. Default 20."),
  );
  props.insert(
    "tool_name".to_string(),
    str_field("Optional tool name filter."),
  );
  primitive_tool(
    "tool_audit_log",
    "Return a recent audit log of tool calls, outputs, source kinds, and approval modes across builtin, MCP, CLI, and API tools.",
    obj(props, &[]),
    default_permissions(),
  )
  .with_permission_key("read")
}

fn list_mcp_resources_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "server".to_string(),
    str_field("Optional MCP server name to filter resources by."),
  );
  primitive_tool(
    "list_mcp_resources",
    "List discovered MCP resources across connected servers. Use this before reading a resource when a workflow needs context from MCP resources.",
    obj(props, &[]),
    default_permissions(),
  )
  .with_permission_key("mcp_resource")
}

fn list_mcp_resource_templates_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "server".to_string(),
    str_field("Optional MCP server name to filter resource templates by."),
  );
  primitive_tool(
    "list_mcp_resource_templates",
    "List discovered MCP resource templates across connected servers.",
    obj(props, &[]),
    default_permissions(),
  )
  .with_permission_key("mcp_resource")
}

fn read_mcp_resource_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "server".to_string(),
    str_field("The MCP server that owns the resource."),
  );
  props.insert(
    "uri".to_string(),
    str_field("The exact MCP resource URI to read."),
  );
  primitive_tool(
    "read_mcp_resource",
    "Read an MCP resource by server and URI. Use this to gather structured context before deciding whether to call mutating tools.",
    obj(props, &["server", "uri"]),
    default_permissions(),
  )
  .with_permission_key("mcp_resource")
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
  primitive_tool(
    "request_user_input",
    "Request user input for one to three short questions and wait for the response.",
    obj(props, &["questions"]),
    default_permissions(),
  )
  .with_permission_key("user_input")
  .with_supports_parallel(false)
  .with_mutates_state(true)
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
      "Response format: text for HTML converted to plain text, html for raw HTML, or raw for the unprocessed response body.",
    ),
  );
  props.insert(
    "timeout".to_string(),
    int_field("Optional timeout in seconds. Default 30, max 120."),
  );
  primitive_tool(
    "web_fetch",
    "Fetch content from a URL. Use this to read web documentation, API references, or other online resources.",
    obj(props, &["url"]),
    ToolPermissions {
      requires_approval: true,
      allow_network: true,
      allow_fs_write: false,
    },
  )
  .with_permission_key("web")
}

fn view_image_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "path".to_string(),
    str_field("Local filesystem path to an image file."),
  );
  primitive_tool(
    "view_image",
    "View a local image from the filesystem. Only use if given a full filepath by the user.",
    obj(props, &["path"]),
    default_permissions(),
  )
  .with_permission_key("read")
}

fn web_search_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "query".to_string(),
    str_field("The search query to look up on the web."),
  );
  props.insert(
    "num_results".to_string(),
    int_field("Number of results to return. Default 8, max 20."),
  );
  props.insert(
    "livecrawl".to_string(),
    str_field("Live crawl mode for the Exa backend: fallback or preferred."),
  );
  props.insert(
    "context_max_characters".to_string(),
    int_field("Maximum characters for context string. Exa backend default 10000."),
  );
  primitive_tool(
    "web_search",
    "Search the web for information using the configured backend and return titles, URLs, and summaries.",
    obj(props, &["query"]),
    ToolPermissions {
      requires_approval: true,
      allow_network: true,
      allow_fs_write: false,
    },
  )
  .with_permission_key("web")
}

fn save_memory_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "fact".to_string(),
    str_field("The clear, self-contained fact or preference to remember."),
  );
  primitive_tool(
    "save_memory",
    "Save a piece of information to persistent memory under ~/.cokra/memory.md.",
    obj(props, &["fact"]),
    ToolPermissions {
      requires_approval: true,
      allow_network: false,
      allow_fs_write: true,
    },
  )
  .with_permission_key("memory")
}

fn diagnostics_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "path".to_string(),
    str_field("Absolute or relative path to the source file to get diagnostics for."),
  );
  props.insert(
    "max_diagnostics".to_string(),
    int_field("Maximum number of diagnostics to return. Default 50."),
  );
  primitive_tool(
    "diagnostics",
    "Get LSP diagnostics for a source file by invoking the appropriate language server on PATH.",
    obj(props, &["path"]),
    default_permissions(),
  )
  .with_permission_key("read")
}

pub fn skill_tool_with_description(description: impl Into<String>) -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "name".to_string(),
    str_field("The skill name from the available_skills list."),
  );
  primitive_tool(
    "skill",
    description,
    obj(props, &["name"]),
    default_permissions(),
  )
  .with_permission_key("skill")
}

fn skill_tool() -> ToolSpec {
  skill_tool_with_description(
    "Load a specialized skill that provides domain-specific instructions and workflows. The full skill content is injected into the conversation inside a <skill_content name=\"...\"> block.",
  )
}

fn read_many_files_tool() -> ToolSpec {
  let mut props = BTreeMap::new();
  props.insert(
    "paths".to_string(),
    JsonSchema::Array {
      items: Box::new(str_field("Absolute path to a file to read.")),
      description: Some("List of absolute file paths to read. Maximum 20.".to_string()),
    },
  );
  props.insert(
    "offset".to_string(),
    int_field("Starting line number for every file. One-indexed, default 1."),
  );
  props.insert(
    "limit".to_string(),
    int_field("Maximum number of lines to read per file. Default 2000."),
  );
  primitive_tool(
    "read_many_files",
    "Read multiple files in one call. Output is grouped by file path and each line is prefixed with its line number.",
    obj(props, &["paths"]),
    default_permissions(),
  )
  .with_permission_key("read")
}

fn todo_write_tool() -> ToolSpec {
  let mut todo_item_props = BTreeMap::new();
  todo_item_props.insert("id".to_string(), str_field("Stable todo item identifier."));
  todo_item_props.insert(
    "content".to_string(),
    str_field("Non-empty task description."),
  );
  todo_item_props.insert(
    "status".to_string(),
    str_field("Task status: pending, in_progress, completed, or cancelled."),
  );
  todo_item_props.insert(
    "priority".to_string(),
    str_field("Task priority: high, medium, or low. Default medium."),
  );

  let todo_item_schema = JsonSchema::Object {
    properties: todo_item_props,
    required: Some(vec![
      "id".to_string(),
      "content".to_string(),
      "status".to_string(),
    ]),
    additional_properties: Some(false.into()),
  };

  let mut props = BTreeMap::new();
  props.insert(
    "todos".to_string(),
    JsonSchema::Array {
      items: Box::new(todo_item_schema),
      description: Some("The complete todo list to write atomically.".to_string()),
    },
  );

  primitive_tool(
    "todo_write",
    "Replace the persisted todo list stored under ~/.cokra/todos.json. Use this to create, update, or clear the task plan.",
    obj(props, &["todos"]),
    ToolPermissions {
      requires_approval: false,
      allow_network: false,
      allow_fs_write: true,
    },
  )
  .with_permission_key("todo")
}
