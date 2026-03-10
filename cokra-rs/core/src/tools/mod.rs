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

/// Build a default tool registry and router from configuration.
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

  let registry = Arc::new(registry);
  let validator = Arc::new(ToolValidator::new(
    config.sandbox.clone(),
    config.approval.clone(),
  ));
  let router = Arc::new(ToolRouter::new(registry.clone(), validator));

  Ok((registry, router))
}
