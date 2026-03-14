use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::tool_runtime::ToolRuntimeCatalog;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct InspectToolHandler {
  catalog: Arc<ToolRuntimeCatalog>,
}

#[derive(Debug, Deserialize)]
struct InspectToolArgs {
  name: String,
}

impl InspectToolHandler {
  pub fn new(catalog: Arc<ToolRuntimeCatalog>) -> Self {
    Self { catalog }
  }
}

#[async_trait]
impl ToolHandler for InspectToolHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
    false
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: InspectToolArgs = invocation.parse_arguments()?;
    let name = args.name.trim();
    if name.is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "name must not be empty".to_string(),
      ));
    }

    let entry = self
      .catalog
      .inspect(name)
      .ok_or_else(|| FunctionCallError::RespondToModel(format!("unknown tool: {name}")))?;
    let content = serde_json::to_string(&entry).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize inspect_tool result: {err}"))
    })?;
    Ok(ToolOutput::success(content).with_id(invocation.id))
  }
}

#[cfg(test)]
mod tests {
  use std::collections::BTreeMap;
  use std::sync::Arc;

  use super::*;
  use crate::tool_runtime::BuiltinToolProvider;
  use crate::tool_runtime::ToolProvider;
  use crate::tools::context::ToolPayload;
  use crate::tools::registry::ToolRegistry;
  use crate::tools::spec::JsonSchema;
  use crate::tools::spec::ToolHandlerType;
  use crate::tools::spec::ToolPermissions;
  use crate::tools::spec::ToolSpec;

  #[tokio::test]
  async fn inspect_tool_returns_capability_facets() {
    let mut registry = ToolRegistry::new();
    registry.register_spec(ToolSpec::new(
      "web_search",
      "Search the web and return structured results.",
      JsonSchema::Object {
        properties: BTreeMap::from([(
          "query".to_string(),
          JsonSchema::String { description: None },
        )]),
        required: Some(vec!["query".to_string()]),
        additional_properties: Some(false.into()),
      },
      None,
      ToolHandlerType::Function,
      ToolPermissions {
        requires_approval: true,
        allow_network: true,
        allow_fs_write: false,
      },
    ));

    let provider: Arc<dyn ToolProvider> = Arc::new(BuiltinToolProvider::from_registry(&registry));
    let catalog = Arc::new(
      crate::tool_runtime::ToolRuntimeCatalog::from_providers(&[provider])
        .await
        .expect("catalog builds"),
    );
    let handler = InspectToolHandler::new(catalog);

    let output = handler
      .handle_async(ToolInvocation {
        id: "inspect-1".to_string(),
        name: "inspect_tool".to_string(),
        payload: ToolPayload::Function {
          arguments: serde_json::json!({
            "name": "web_search"
          })
          .to_string(),
        },
        cwd: std::env::temp_dir(),
        runtime: None,
      })
      .await
      .expect("inspect succeeds");

    let parsed: serde_json::Value =
      serde_json::from_str(&output.text_content()).expect("valid json");
    let backends = parsed["capabilities"]["network_backends"]
      .as_array()
      .expect("backend array")
      .iter()
      .filter_map(serde_json::Value::as_str)
      .collect::<Vec<_>>();
    assert!(backends.contains(&"provider_native_openai_codex"));
    assert!(backends.contains(&"local_exa"));
  }
}
