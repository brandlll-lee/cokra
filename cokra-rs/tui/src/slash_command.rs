/// Commands that can be invoked by starting a message with a leading slash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SlashCommand {
  // Keep order aligned with codex popup presentation order.
  Model,
  Approvals,
  Permissions,
  ElevateSandbox,
  SandboxReadRoot,
  Experimental,
  Skills,
  Review,
  Rename,
  New,
  Resume,
  Fork,
  Init,
  Compact,
  Plan,
  Collab,
  Agent,
  Diff,
  Mention,
  Status,
  DebugConfig,
  Statusline,
  Mcp,
  Apps,
  Logout,
  Quit,
  Exit,
  Feedback,
  Rollout,
  Ps,
  Clean,
  Personality,
  TestApproval,
  MemoryDrop,
  MemoryUpdate,
}

impl SlashCommand {
  pub(crate) fn command(self) -> &'static str {
    match self {
      SlashCommand::Model => "model",
      SlashCommand::Approvals => "approvals",
      SlashCommand::Permissions => "permissions",
      SlashCommand::ElevateSandbox => "setup-default-sandbox",
      SlashCommand::SandboxReadRoot => "sandbox-add-read-dir",
      SlashCommand::Experimental => "experimental",
      SlashCommand::Skills => "skills",
      SlashCommand::Review => "review",
      SlashCommand::Rename => "rename",
      SlashCommand::New => "new",
      SlashCommand::Resume => "resume",
      SlashCommand::Fork => "fork",
      SlashCommand::Init => "init",
      SlashCommand::Compact => "compact",
      SlashCommand::Plan => "plan",
      SlashCommand::Collab => "collab",
      SlashCommand::Agent => "agent",
      SlashCommand::Diff => "diff",
      SlashCommand::Mention => "mention",
      SlashCommand::Status => "status",
      SlashCommand::DebugConfig => "debug-config",
      SlashCommand::Statusline => "statusline",
      SlashCommand::Mcp => "mcp",
      SlashCommand::Apps => "apps",
      SlashCommand::Logout => "logout",
      SlashCommand::Quit => "quit",
      SlashCommand::Exit => "exit",
      SlashCommand::Feedback => "feedback",
      SlashCommand::Rollout => "rollout",
      SlashCommand::Ps => "ps",
      SlashCommand::Clean => "clean",
      SlashCommand::Personality => "personality",
      SlashCommand::TestApproval => "test-approval",
      SlashCommand::MemoryDrop => "debug-m-drop",
      SlashCommand::MemoryUpdate => "debug-m-update",
    }
  }

  /// User-visible description shown in the popup.
  pub(crate) fn description(self) -> &'static str {
    match self {
      SlashCommand::Feedback => "send logs to maintainers",
      SlashCommand::New => "start a new chat during a conversation",
      SlashCommand::Init => "create an AGENTS.md file with instructions for cokra",
      SlashCommand::Compact => "summarize conversation to prevent hitting the context limit",
      SlashCommand::Review => "review my current changes and find issues",
      SlashCommand::Rename => "rename the current thread",
      SlashCommand::Resume => "resume a saved chat",
      SlashCommand::Fork => "fork the current chat",
      SlashCommand::Quit | SlashCommand::Exit => "exit cokra",
      SlashCommand::Diff => "show git diff (including untracked files)",
      SlashCommand::Mention => "mention a file",
      SlashCommand::Skills => "use skills to improve how cokra performs specific tasks",
      SlashCommand::Status => "show current session configuration and token usage",
      SlashCommand::DebugConfig => "show config layers and requirement sources for debugging",
      SlashCommand::Statusline => "configure which items appear in the status line",
      SlashCommand::Ps => "list background terminals",
      SlashCommand::Clean => "stop all background terminals",
      SlashCommand::MemoryDrop => "DO NOT USE",
      SlashCommand::MemoryUpdate => "DO NOT USE",
      SlashCommand::Model => "choose what model and reasoning effort to use",
      SlashCommand::Personality => "choose a communication style for cokra",
      SlashCommand::Plan => "switch to Plan mode",
      SlashCommand::Collab => "change collaboration mode",
      SlashCommand::Agent => "switch the active agent thread",
      SlashCommand::Approvals => "choose what cokra is allowed to do",
      SlashCommand::Permissions => "choose what cokra is allowed to do",
      SlashCommand::ElevateSandbox => "set up elevated agent sandbox",
      SlashCommand::SandboxReadRoot => {
        "let sandbox read a directory: /sandbox-add-read-dir <absolute_path>"
      }
      SlashCommand::Experimental => "toggle experimental features",
      SlashCommand::Mcp => "list configured MCP tools",
      SlashCommand::Apps => "manage apps",
      SlashCommand::Logout => "log out of cokra",
      SlashCommand::Rollout => "print the rollout file path",
      SlashCommand::TestApproval => "test approval request",
    }
  }

  /// Whether this command supports inline args (e.g. `/review ...`).
  pub(crate) fn supports_inline_args(self) -> bool {
    matches!(
      self,
      SlashCommand::Review
        | SlashCommand::Rename
        | SlashCommand::Plan
        | SlashCommand::SandboxReadRoot
    )
  }

  /// Whether this command can be run while a task is in progress.
  pub(crate) fn available_during_task(self) -> bool {
    match self {
      SlashCommand::New
      | SlashCommand::Resume
      | SlashCommand::Fork
      | SlashCommand::Init
      | SlashCommand::Compact
      | SlashCommand::Model
      | SlashCommand::Personality
      | SlashCommand::Approvals
      | SlashCommand::Permissions
      | SlashCommand::ElevateSandbox
      | SlashCommand::SandboxReadRoot
      | SlashCommand::Experimental
      | SlashCommand::Review
      | SlashCommand::Plan
      | SlashCommand::Logout
      | SlashCommand::MemoryDrop
      | SlashCommand::MemoryUpdate
      | SlashCommand::Statusline => false,
      SlashCommand::Diff
      | SlashCommand::Rename
      | SlashCommand::Mention
      | SlashCommand::Skills
      | SlashCommand::Status
      | SlashCommand::DebugConfig
      | SlashCommand::Ps
      | SlashCommand::Clean
      | SlashCommand::Mcp
      | SlashCommand::Apps
      | SlashCommand::Feedback
      | SlashCommand::Quit
      | SlashCommand::Exit
      | SlashCommand::Rollout
      | SlashCommand::TestApproval
      | SlashCommand::Collab
      | SlashCommand::Agent => true,
    }
  }

  fn is_visible(self) -> bool {
    match self {
      SlashCommand::SandboxReadRoot => cfg!(target_os = "windows"),
      SlashCommand::Rollout | SlashCommand::TestApproval => cfg!(debug_assertions),
      _ => true,
    }
  }
}

const ALL_SLASH_COMMANDS: &[SlashCommand] = &[
  SlashCommand::Model,
  SlashCommand::Approvals,
  SlashCommand::Permissions,
  SlashCommand::ElevateSandbox,
  SlashCommand::SandboxReadRoot,
  SlashCommand::Experimental,
  SlashCommand::Skills,
  SlashCommand::Review,
  SlashCommand::Rename,
  SlashCommand::New,
  SlashCommand::Resume,
  SlashCommand::Fork,
  SlashCommand::Init,
  SlashCommand::Compact,
  SlashCommand::Plan,
  SlashCommand::Collab,
  SlashCommand::Agent,
  SlashCommand::Diff,
  SlashCommand::Mention,
  SlashCommand::Status,
  SlashCommand::DebugConfig,
  SlashCommand::Statusline,
  SlashCommand::Mcp,
  SlashCommand::Apps,
  SlashCommand::Logout,
  SlashCommand::Quit,
  SlashCommand::Exit,
  SlashCommand::Feedback,
  SlashCommand::Rollout,
  SlashCommand::Ps,
  SlashCommand::Clean,
  SlashCommand::Personality,
  SlashCommand::TestApproval,
  SlashCommand::MemoryDrop,
  SlashCommand::MemoryUpdate,
];

pub(crate) fn parse_builtin(name: &str) -> Option<SlashCommand> {
  built_in_slash_commands()
    .into_iter()
    .find(|(command_name, _)| *command_name == name)
    .map(|(_, cmd)| cmd)
}

/// Return all built-in commands in presentation order.
pub(crate) fn built_in_slash_commands() -> Vec<(&'static str, SlashCommand)> {
  ALL_SLASH_COMMANDS
    .iter()
    .copied()
    .filter(|command| command.is_visible())
    .map(|command| (command.command(), command))
    .collect()
}
