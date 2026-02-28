pub mod context;
pub mod handlers;
pub mod registry;
pub mod router;
pub mod spec;
pub mod validation;

use std::sync::Arc;

use cokra_config::Config;

use crate::tools::registry::ToolRegistry;
use crate::tools::router::ToolRouter;
use crate::tools::spec::build_specs;
use crate::tools::validation::ToolValidator;

/// Build a default tool registry and router from configuration.
pub fn build_default_tools(config: &Config) -> (Arc<ToolRegistry>, Arc<ToolRouter>) {
  let mut registry = ToolRegistry::new();

  for spec in build_specs() {
    registry.register_spec(spec);
  }

  handlers::register_builtin_handlers(&mut registry);

  let registry = Arc::new(registry);
  let validator = Arc::new(ToolValidator::new(
    config.sandbox.clone(),
    config.approval.clone(),
  ));
  let router = Arc::new(ToolRouter::new(registry.clone(), validator));

  (registry, router)
}
