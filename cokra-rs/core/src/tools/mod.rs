//! The Cokra tool kernel.
//!
//! This module is the main kernel entry for tool execution:
//! - [`registry`] stores tool definitions and handlers
//! - [`router`] validates and executes calls
//! - [`spec`] defines the built-in tool contracts
//!
//! The remaining submodules are crate-internal implementation details.

pub(crate) mod command_intent;
pub(crate) mod context;
pub(crate) mod diff_tracker;
pub(crate) mod events;
pub(crate) mod handlers;
pub(crate) mod hooks;
pub(crate) mod network_approval;
pub(crate) mod orchestrator;
pub(crate) mod parallel;
pub mod registry;
pub mod router;
pub(crate) mod runtimes;
pub(crate) mod sandboxing;
pub mod spec;
pub(crate) mod validation;

use std::sync::Arc;

use cokra_config::Config;
use cokra_config::ExecBackend;
use cokra_config::ExecPublicSurface;

use crate::integrations::discover_integrations;
use crate::integrations::manifest::IntegrationKind;
use crate::integrations::project_integrations;
use crate::integrations::projected_tool_definitions;
use crate::mcp::McpConnectionManager;
use crate::skills::loader::build_skill_tool_description;
use crate::tool_runtime::ApiToolProvider;
use crate::tool_runtime::BuiltinToolProvider;
use crate::tool_runtime::CliToolProvider;
use crate::tool_runtime::McpToolProvider;
use crate::tool_runtime::ToolProvider;
use crate::tool_runtime::ToolRuntimeCatalog;
use crate::tool_runtime::UnifiedToolRuntime;
use crate::tools::spec::build_specs;
use crate::tools::spec::skill_tool_with_description;

pub use context::FunctionCallError;
pub use context::ToolInvocation;
pub use context::ToolOutput;
pub use registry::ToolHandler;
pub use registry::ToolKind;
pub use registry::ToolRegistry;
pub use router::ToolRouter;
pub use router::ToolRunContext;
pub use spec::JsonSchema;
pub use spec::ToolHandlerType;
pub use spec::ToolPermissions;
pub use spec::ToolSourceKind;
pub use spec::ToolSpec;
pub use validation::ToolValidator;

pub const SHELL_TOOL_NAME: &str = "shell";
pub const UNIFIED_EXEC_TOOL_NAME: &str = "unified_exec";
pub const LOCAL_SHELL_TOOL_ALIAS: &str = "local_shell";
pub const CONTAINER_EXEC_TOOL_ALIAS: &str = "container.exec";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedExecBackend {
  ShellCommand,
  UnifiedExec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedExecToolConfig {
  pub public_surface: &'static str,
  pub backend: ResolvedExecBackend,
}

pub(crate) struct DefaultToolingBundle {
  pub registry: Arc<ToolRegistry>,
  pub router: Arc<ToolRouter>,
  pub mcp_manager: Arc<McpConnectionManager>,
  pub runtime: Arc<UnifiedToolRuntime>,
}

pub fn canonical_exec_tool_name(name: &str) -> Option<&'static str> {
  match name {
    SHELL_TOOL_NAME => Some(SHELL_TOOL_NAME),
    UNIFIED_EXEC_TOOL_NAME | LOCAL_SHELL_TOOL_ALIAS | CONTAINER_EXEC_TOOL_ALIAS => {
      Some(UNIFIED_EXEC_TOOL_NAME)
    }
    _ => None,
  }
}

pub fn is_exec_tool_name(name: &str) -> bool {
  canonical_exec_tool_name(name).is_some()
}

pub fn resolve_exec_tool_config(config: &Config) -> ResolvedExecToolConfig {
  let public_surface = match config.tools.exec.public_surface {
    ExecPublicSurface::Auto | ExecPublicSurface::Shell => SHELL_TOOL_NAME,
    ExecPublicSurface::UnifiedExec => UNIFIED_EXEC_TOOL_NAME,
  };

  let backend = match config.tools.exec.backend {
    ExecBackend::Auto => {
      if public_surface == UNIFIED_EXEC_TOOL_NAME {
        ResolvedExecBackend::UnifiedExec
      } else {
        ResolvedExecBackend::ShellCommand
      }
    }
    ExecBackend::ShellCommand => ResolvedExecBackend::ShellCommand,
    ExecBackend::UnifiedExec => ResolvedExecBackend::UnifiedExec,
  };

  ResolvedExecToolConfig {
    public_surface,
    backend,
  }
}

fn register_default_aliases(registry: &mut ToolRegistry) {
  registry.register_alias(LOCAL_SHELL_TOOL_ALIAS, UNIFIED_EXEC_TOOL_NAME);
  registry.register_alias(CONTAINER_EXEC_TOOL_ALIAS, UNIFIED_EXEC_TOOL_NAME);
}

/// Returns true when the configured model prefers the freeform `apply_patch`
/// tool over the structured `edit_file` tool.
///
/// Mirrors OpenCode's heuristic: GPT-series models (excluding gpt-4 and
/// "-oss" variants) work best with `apply_patch`; all other models
/// (Claude, Gemini, open-source) prefer `edit_file` + `write_file`.
fn model_prefers_apply_patch(config: &Config) -> bool {
  let model = config.models.model.to_lowercase();
  model.contains("gpt-") && !model.contains("-oss") && !model.contains("gpt-4")
}

/// Build the tool kernel from configuration.
///
/// Automatically selects either `edit_file` or `apply_patch` based on the
/// configured model, following the OpenCode pattern where GPT-style models use
/// `apply_patch` and other models use `edit_file` + `write_file`.
pub async fn build_default_tools(
  config: &Config,
) -> anyhow::Result<(Arc<ToolRegistry>, Arc<ToolRouter>)> {
  let bundle =
    build_default_tooling_with_cwd(config, &std::env::current_dir().unwrap_or_default()).await?;
  Ok((bundle.registry, bundle.router))
}

/// Internal variant that accepts an explicit cwd for tests and runtime bootstrapping.
pub(crate) async fn build_default_tools_with_cwd(
  config: &Config,
  cwd: &std::path::Path,
) -> anyhow::Result<(Arc<ToolRegistry>, Arc<ToolRouter>)> {
  let bundle = build_default_tooling_with_cwd(config, cwd).await?;
  Ok((bundle.registry, bundle.router))
}

pub(crate) async fn build_default_tooling_with_cwd(
  config: &Config,
  cwd: &std::path::Path,
) -> anyhow::Result<DefaultToolingBundle> {
  let mut registry = ToolRegistry::new();
  let integration_catalog = discover_integrations(cwd).await;
  for warning in &integration_catalog.warnings {
    tracing::warn!("{warning}");
  }
  let projected_integrations = project_integrations(&config.mcp, &integration_catalog)?;
  let mcp_manager =
    Arc::new(McpConnectionManager::new(&projected_integrations.effective_mcp).await?);
  let exec_config = resolve_exec_tool_config(config);

  // Mirrors OpenCode's skill-tool pattern: compute the cwd-aware skill
  // description before registering specs so the synthetic `skill` entry reflects
  // the prompt assets actually available in this workspace.
  let skill_description = build_skill_tool_description(cwd).await;

  for spec in build_specs() {
    if spec.name == "skill" {
      // Override the static skill spec with the dynamically generated description.
      registry.register_spec(skill_tool_with_description(&skill_description));
    } else {
      registry.register_spec(spec);
    }
  }
  for spec in mcp_manager.tool_specs() {
    registry.register_spec(spec);
  }
  for tool in projected_integrations
    .cli_tools
    .iter()
    .chain(projected_integrations.api_tools.iter())
  {
    registry.register_tool(tool.spec.clone(), Arc::clone(&tool.handler));
    for alias in &tool.definition.aliases {
      registry.register_alias(alias.clone(), tool.spec.name.clone());
    }
  }

  register_default_aliases(&mut registry);

  handlers::register_builtin_handlers(&mut registry, Arc::clone(&mcp_manager));

  // Model-based tool selection: GPT-codex models prefer apply_patch,
  // all other models prefer edit_file + write_file.
  if model_prefers_apply_patch(config) {
    registry.exclude_tool("edit_file");
  } else {
    registry.exclude_tool("apply_patch");
  }

  if exec_config.public_surface == SHELL_TOOL_NAME {
    registry.exclude_tool(UNIFIED_EXEC_TOOL_NAME);
  } else {
    registry.exclude_tool(SHELL_TOOL_NAME);
  }

  let cli_definitions = projected_tool_definitions(&projected_integrations, IntegrationKind::Cli);
  let api_definitions = projected_tool_definitions(&projected_integrations, IntegrationKind::Api);
  let providers: Vec<Arc<dyn ToolProvider>> = vec![
    Arc::new(BuiltinToolProvider::from_registry(&registry)),
    Arc::new(McpToolProvider::from_manager(Arc::clone(&mcp_manager))),
    Arc::new(CliToolProvider::new("cli_integrations", cli_definitions)),
    Arc::new(ApiToolProvider::new("api_integrations", api_definitions)),
  ];
  let tool_catalog = Arc::new(ToolRuntimeCatalog::from_providers(&providers).await?);
  registry.register_handler(
    "search_tool",
    Arc::new(handlers::dynamic::DynamicToolHandler::new(Arc::clone(
      &tool_catalog,
    ))),
  );
  registry.register_handler(
    "inspect_tool",
    Arc::new(handlers::inspect_tool::InspectToolHandler::new(Arc::clone(
      &tool_catalog,
    ))),
  );

  let registry = Arc::new(registry);
  let validator = Arc::new(ToolValidator::new(
    config.sandbox.clone(),
    config.approval.clone(),
  ));
  let router = Arc::new(ToolRouter::new_with_exec_config(
    registry.clone(),
    validator,
    exec_config,
  ));
  let runtime = Arc::new(UnifiedToolRuntime::new(
    Arc::clone(&tool_catalog),
    providers,
    Arc::clone(&router),
  ));

  Ok(DefaultToolingBundle {
    registry,
    router,
    mcp_manager,
    runtime,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  fn config_with_model(model: &str) -> Config {
    let mut config = Config::default();
    config.models.model = model.to_string();
    config
  }

  #[test]
  fn gpt_codex_prefers_apply_patch() {
    assert!(model_prefers_apply_patch(&config_with_model(
      "gpt-5.2-codex"
    )));
  }

  #[test]
  fn gpt_5_prefers_apply_patch() {
    assert!(model_prefers_apply_patch(&config_with_model("gpt-5")));
  }

  #[test]
  fn gpt_oss_prefers_edit() {
    assert!(!model_prefers_apply_patch(&config_with_model("gpt-5-oss")));
  }

  #[test]
  fn gpt4_prefers_edit() {
    assert!(!model_prefers_apply_patch(&config_with_model("gpt-4o")));
  }

  #[test]
  fn claude_prefers_edit() {
    assert!(!model_prefers_apply_patch(&config_with_model(
      "claude-sonnet-4-20250514"
    )));
  }

  #[test]
  fn gemini_prefers_edit() {
    assert!(!model_prefers_apply_patch(&config_with_model(
      "gemini-2.5-pro"
    )));
  }

  #[test]
  fn deepseek_prefers_edit() {
    assert!(!model_prefers_apply_patch(&config_with_model(
      "deepseek-r1"
    )));
  }

  #[test]
  fn case_insensitive_model_match() {
    assert!(model_prefers_apply_patch(&config_with_model(
      "GPT-5.2-Codex"
    )));
    assert!(!model_prefers_apply_patch(&config_with_model("GPT-4o")));
  }

  #[test]
  fn default_exec_config_exposes_shell_surface() {
    let resolved = resolve_exec_tool_config(&Config::default());
    assert_eq!(resolved.public_surface, SHELL_TOOL_NAME);
    assert_eq!(resolved.backend, ResolvedExecBackend::ShellCommand);
  }

  #[test]
  fn unified_exec_surface_defaults_to_unified_backend() {
    let mut config = Config::default();
    config.tools.exec.public_surface = ExecPublicSurface::UnifiedExec;

    let resolved = resolve_exec_tool_config(&config);
    assert_eq!(resolved.public_surface, UNIFIED_EXEC_TOOL_NAME);
    assert_eq!(resolved.backend, ResolvedExecBackend::UnifiedExec);
  }

  #[test]
  fn explicit_exec_backend_override_is_respected() {
    let mut config = Config::default();
    config.tools.exec.backend = ExecBackend::UnifiedExec;

    let resolved = resolve_exec_tool_config(&config);
    assert_eq!(resolved.public_surface, SHELL_TOOL_NAME);
    assert_eq!(resolved.backend, ResolvedExecBackend::UnifiedExec);
  }
}
