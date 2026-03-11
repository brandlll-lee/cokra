pub mod context;
pub mod events;
pub mod handlers;
pub mod network_approval;
pub mod orchestrator;
pub mod parallel;
pub mod registry;
pub mod router;
pub mod runtimes;
pub mod sandboxing;
pub mod spec;
pub mod validation;

use std::sync::Arc;

use cokra_config::Config;

use crate::mcp::McpConnectionManager;
use crate::tools::registry::ToolRegistry;
use crate::tools::router::ToolRouter;
use crate::tools::spec::build_specs;
use crate::tools::validation::ToolValidator;

/// Returns true when the configured model prefers the freeform `apply_patch`
/// tool over the structured `edit_file` tool.
///
/// Mirrors OpenCode's heuristic: GPT-series models (excluding gpt-4 and
/// "-oss" variants) work best with `apply_patch`; all other models
/// (Claude, Gemini, open-source) prefer `edit_file` + `write_file`.
fn model_prefers_apply_patch(config: &Config) -> bool {
  let model = config.models.model.to_lowercase();
  model.contains("gpt-")
    && !model.contains("-oss")
    && !model.contains("gpt-4")
}

/// Build a default tool registry and router from configuration.
///
/// Automatically selects either `edit_file` or `apply_patch` based on the
/// configured model, following the OpenCode pattern where GPT-codex models
/// use `apply_patch` and all other models use `edit_file` + `write_file`.
pub async fn build_default_tools(
  config: &Config,
) -> anyhow::Result<(Arc<ToolRegistry>, Arc<ToolRouter>)> {
  let mut registry = ToolRegistry::new();
  let mcp_manager = Arc::new(McpConnectionManager::new(&config.mcp).await?);

  for spec in build_specs() {
    registry.register_spec(spec);
  }
  for spec in mcp_manager.tool_specs() {
    registry.register_spec(spec);
  }

  handlers::register_builtin_handlers(&mut registry, mcp_manager);

  // Model-based tool selection: GPT-codex models prefer apply_patch,
  // all other models prefer edit_file + write_file.
  if model_prefers_apply_patch(config) {
    registry.exclude_tool("edit_file");
  } else {
    registry.exclude_tool("apply_patch");
  }

  let registry = Arc::new(registry);
  let validator = Arc::new(ToolValidator::new(
    config.sandbox.clone(),
    config.approval.clone(),
  ));
  let router = Arc::new(ToolRouter::new(registry.clone(), validator));

  Ok((registry, router))
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
    assert!(model_prefers_apply_patch(&config_with_model("gpt-5.2-codex")));
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
    assert!(!model_prefers_apply_patch(&config_with_model("claude-sonnet-4-20250514")));
  }

  #[test]
  fn gemini_prefers_edit() {
    assert!(!model_prefers_apply_patch(&config_with_model("gemini-2.5-pro")));
  }

  #[test]
  fn deepseek_prefers_edit() {
    assert!(!model_prefers_apply_patch(&config_with_model("deepseek-r1")));
  }

  #[test]
  fn case_insensitive_model_match() {
    assert!(model_prefers_apply_patch(&config_with_model("GPT-5.2-Codex")));
    assert!(!model_prefers_apply_patch(&config_with_model("GPT-4o")));
  }
}
