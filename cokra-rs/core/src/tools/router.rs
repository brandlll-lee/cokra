use std::sync::Arc;

use serde_json::Value;
use uuid::Uuid;

use crate::tools::context::{FunctionCallError, ToolInvocation, ToolOutput};
use crate::tools::registry::ToolRegistry;
use crate::tools::spec::ToolSpec;
use crate::tools::validation::{ToolCall, ToolValidator};

pub struct ToolRouter {
  registry: Arc<ToolRegistry>,
  validator: Arc<ToolValidator>,
}

impl ToolRouter {
  pub fn new(registry: Arc<ToolRegistry>, validator: Arc<ToolValidator>) -> Self {
    Self {
      registry,
      validator,
    }
  }

  pub async fn route_tool_call(
    &self,
    tool_name: &str,
    arguments: Value,
  ) -> Result<ToolOutput, FunctionCallError> {
    let call = ToolCall {
      tool_name: tool_name.to_string(),
      args: arguments.clone(),
    };

    let validation = self.validator.validate_tool_call(&call)?;
    if !validation.valid {
      let reason = validation
        .reason
        .unwrap_or_else(|| format!("tool {tool_name} requires user approval"));
      return Err(FunctionCallError::PermissionDenied(reason));
    }

    let invocation = ToolInvocation {
      id: Uuid::new_v4().to_string(),
      name: tool_name.to_string(),
      arguments: arguments.to_string(),
    };

    self.registry.dispatch(invocation)
  }

  pub fn list_available_tools(&self) -> Vec<ToolSpec> {
    self.registry.list_specs()
  }

  pub fn registry(&self) -> Arc<ToolRegistry> {
    self.registry.clone()
  }
}
