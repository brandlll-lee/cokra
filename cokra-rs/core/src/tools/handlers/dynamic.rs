use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::tool_runtime::ToolCatalogMatch;
use crate::tool_runtime::ToolRuntimeCatalog;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct DynamicToolHandler {
  catalog: Arc<ToolRuntimeCatalog>,
}

const DEFAULT_LIMIT: usize = 8;

#[derive(Debug, Deserialize)]
struct SearchArgs {
  query: String,
  #[serde(default = "default_limit")]
  limit: usize,
}

#[derive(Debug, Serialize)]
struct SearchToolResponse {
  query: String,
  total_matches: usize,
  results: Vec<ToolCatalogMatch>,
}

fn default_limit() -> usize {
  DEFAULT_LIMIT
}

impl DynamicToolHandler {
  pub fn new(catalog: Arc<ToolRuntimeCatalog>) -> Self {
    Self { catalog }
  }
}

#[async_trait]
impl ToolHandler for DynamicToolHandler {
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
    let args: SearchArgs = invocation.parse_arguments()?;
    let query = args.query.trim();
    if query.is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "query must not be empty".to_string(),
      ));
    }
    if args.limit == 0 {
      return Err(FunctionCallError::RespondToModel(
        "limit must be greater than zero".to_string(),
      ));
    }

    let results = self.catalog.search(query, args.limit.min(20));
    let response = SearchToolResponse {
      query: query.to_string(),
      total_matches: results.len(),
      results,
    };
    let content = serde_json::to_string(&response).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize search_tool result: {err}"))
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
  use async_trait::async_trait;

  struct DummyHandler;

  #[async_trait]
  impl ToolHandler for DummyHandler {
    fn kind(&self) -> ToolKind {
      ToolKind::Function
    }

    fn handle(
      &self,
      invocation: ToolInvocation,
    ) -> Result<ToolOutput, crate::tools::context::FunctionCallError> {
      Ok(ToolOutput::success("ok").with_id(invocation.id))
    }
  }

  async fn make_catalog() -> ToolRuntimeCatalog {
    let mut registry = ToolRegistry::new();
    registry.register_tool(
      ToolSpec::new(
        "inspect_tool",
        "Inspect a tool definition and aliases.",
        JsonSchema::Object {
          properties: BTreeMap::from([(
            "name".to_string(),
            JsonSchema::String { description: None },
          )]),
          required: Some(vec!["name".to_string()]),
          additional_properties: Some(false.into()),
        },
        None,
        ToolHandlerType::Function,
        ToolPermissions::default(),
      ),
      Arc::new(DummyHandler),
    );
    registry.register_tool(
      ToolSpec::new(
        "unified_exec",
        "Run a pre-tokenized local command.",
        JsonSchema::Object {
          properties: BTreeMap::from([(
            "command".to_string(),
            JsonSchema::Array {
              items: Box::new(JsonSchema::String { description: None }),
              description: None,
            },
          )]),
          required: Some(vec!["command".to_string()]),
          additional_properties: Some(false.into()),
        },
        None,
        ToolHandlerType::Function,
        ToolPermissions::default(),
      ),
      Arc::new(DummyHandler),
    );
    registry.register_tool(
      ToolSpec::new(
        "lsp",
        "Run semantic code navigation requests.",
        JsonSchema::Object {
          properties: BTreeMap::from([
            (
              "operation".to_string(),
              JsonSchema::String { description: None },
            ),
            (
              "file_path".to_string(),
              JsonSchema::String { description: None },
            ),
          ]),
          required: Some(vec!["operation".to_string(), "file_path".to_string()]),
          additional_properties: Some(false.into()),
        },
        None,
        ToolHandlerType::Function,
        ToolPermissions::default(),
      ),
      Arc::new(DummyHandler),
    );
    registry.register_alias("container.exec", "unified_exec");
    let provider: Arc<dyn ToolProvider> = Arc::new(BuiltinToolProvider::from_registry(&registry));
    ToolRuntimeCatalog::from_providers(&[provider])
      .await
      .expect("catalog builds")
  }

  #[tokio::test]
  async fn search_tool_returns_structured_matches() {
    let handler = DynamicToolHandler::new(Arc::new(make_catalog().await));
    let invocation = ToolInvocation {
      id: "search-1".to_string(),
      name: "search_tool".to_string(),
      payload: ToolPayload::Function {
        arguments: serde_json::json!({
          "query": "container exec command",
          "limit": 5
        })
        .to_string(),
      },
      cwd: std::env::temp_dir(),
      runtime: None,
    };

    let output = handler
      .handle_async(invocation)
      .await
      .expect("search succeeds");
    let parsed: serde_json::Value =
      serde_json::from_str(&output.text_content()).expect("valid json");
    assert_eq!(parsed["query"], "container exec command");
    assert_eq!(parsed["results"][0]["tool"]["id"], "unified_exec");
    assert_eq!(
      parsed["results"][0]["tool"]["capabilities"]["interactive_exec"],
      false
    );
  }

  #[tokio::test]
  async fn search_tool_matches_semantic_lsp_capability_terms() {
    let handler = DynamicToolHandler::new(Arc::new(make_catalog().await));
    let invocation = ToolInvocation {
      id: "search-2".to_string(),
      name: "search_tool".to_string(),
      payload: ToolPayload::Function {
        arguments: serde_json::json!({
          "query": "semantic lsp references call hierarchy",
          "limit": 5
        })
        .to_string(),
      },
      cwd: std::env::temp_dir(),
      runtime: None,
    };

    let output = handler
      .handle_async(invocation)
      .await
      .expect("search succeeds");
    let parsed: serde_json::Value =
      serde_json::from_str(&output.text_content()).expect("valid json");
    let matches = parsed["results"]
      .as_array()
      .expect("results array")
      .iter()
      .filter(|entry| entry["tool"]["id"] == "lsp")
      .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["tool"]["capabilities"]["semantic_lsp"], true);
  }
}
