use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolOutput;
use crate::tools::router::ToolRouter;
use crate::tools::router::ToolRunContext;

use super::ToolCall;
use super::ToolDefinition;
use super::ToolProvider;
use super::ToolResult;
use super::ToolResultMetadata;
use super::ToolSource;

#[derive(Debug, Clone, Serialize)]
pub struct ToolCatalogMatch {
  pub tool: ToolDefinition,
  pub score: f64,
  pub matched_terms: Vec<String>,
}

#[derive(Debug, Clone)]
struct IndexedTool {
  tool: ToolDefinition,
  search_terms: HashMap<String, usize>,
  doc_len: usize,
}

#[derive(Debug, Clone)]
pub struct ToolRuntimeCatalog {
  entries: Vec<IndexedTool>,
  by_name: HashMap<String, usize>,
  document_frequency: HashMap<String, usize>,
  active_count: usize,
  average_doc_len: f64,
}

impl ToolRuntimeCatalog {
  pub fn from_tools(tools: Vec<ToolDefinition>) -> Self {
    let mut entries = Vec::new();
    let mut by_name = HashMap::new();

    for tool in tools {
      let index = entries.len();
      for key in std::iter::once(tool.id.clone())
        .chain(std::iter::once(tool.name.clone()))
        .chain(tool.aliases.iter().cloned())
      {
        by_name.insert(key, index);
      }
      entries.push(IndexedTool::new(tool));
    }

    let active_entries = entries
      .iter()
      .filter(|entry| entry.tool.enabled)
      .collect::<Vec<_>>();
    let active_count = active_entries.len();
    let average_doc_len = if active_count == 0 {
      1.0
    } else {
      active_entries
        .iter()
        .map(|entry| entry.doc_len as f64)
        .sum::<f64>()
        / active_count as f64
    };
    let mut document_frequency = HashMap::new();
    for entry in active_entries {
      for token in entry.search_terms.keys() {
        *document_frequency.entry(token.clone()).or_insert(0) += 1;
      }
    }

    Self {
      entries,
      by_name,
      document_frequency,
      active_count,
      average_doc_len,
    }
  }

  pub async fn from_providers(providers: &[Arc<dyn ToolProvider>]) -> Result<Self> {
    let mut seen = HashSet::new();
    let mut tools = Vec::new();
    for provider in providers {
      for tool in provider.list_tools().await? {
        if seen.insert(tool.id.clone()) {
          tools.push(tool);
        }
      }
    }
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Self::from_tools(tools))
  }

  pub fn definitions(&self) -> Vec<ToolDefinition> {
    self.entries.iter().map(|entry| entry.tool.clone()).collect()
  }

  pub fn inspect(&self, name: &str) -> Option<ToolDefinition> {
    let key = name.trim();
    self
      .by_name
      .get(key)
      .and_then(|index| self.entries.get(*index))
      .map(|entry| entry.tool.clone())
  }

  pub fn search(&self, query: &str, limit: usize) -> Vec<ToolCatalogMatch> {
    let tokens = tokenize(query);
    if tokens.is_empty() || limit == 0 {
      return Vec::new();
    }

    let query_terms = tokens.into_iter().collect::<HashSet<_>>();
    let mut matches = self
      .entries
      .iter()
      .filter(|entry| entry.tool.enabled)
      .filter_map(|entry| {
        let score = bm25_score(
          &query_terms,
          &entry.search_terms,
          entry.doc_len,
          &self.document_frequency,
          self.active_count,
          self.average_doc_len,
        );
        if score <= 0.0 {
          return None;
        }
        let matched_terms = query_terms
          .iter()
          .filter(|term| entry.search_terms.contains_key(*term))
          .cloned()
          .collect::<Vec<_>>();
        Some(ToolCatalogMatch {
          tool: entry.tool.clone(),
          score,
          matched_terms,
        })
      })
      .collect::<Vec<_>>();

    matches.sort_by(|left, right| {
      right
        .score
        .partial_cmp(&left.score)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| left.tool.name.cmp(&right.tool.name))
    });
    matches.truncate(limit);
    matches
  }
}

impl IndexedTool {
  fn new(tool: ToolDefinition) -> Self {
    let search_text = build_search_text(&tool);
    let tokens = tokenize(&search_text);
    let mut search_terms = HashMap::new();
    for token in tokens {
      *search_terms.entry(token).or_insert(0) += 1;
    }
    let doc_len = search_terms.values().sum();
    Self {
      tool,
      search_terms,
      doc_len,
    }
  }
}

pub struct UnifiedToolRuntime {
  catalog: Arc<ToolRuntimeCatalog>,
  providers: Vec<Arc<dyn ToolProvider>>,
  router: Arc<ToolRouter>,
}

impl UnifiedToolRuntime {
  pub fn new(
    catalog: Arc<ToolRuntimeCatalog>,
    providers: Vec<Arc<dyn ToolProvider>>,
    router: Arc<ToolRouter>,
  ) -> Self {
    Self {
      catalog,
      providers,
      router,
    }
  }

  pub fn catalog(&self) -> Arc<ToolRuntimeCatalog> {
    Arc::clone(&self.catalog)
  }

  pub fn providers(&self) -> &[Arc<dyn ToolProvider>] {
    &self.providers
  }

  pub async fn execute(
    &self,
    call: ToolCall,
    ctx: ToolRunContext,
  ) -> Result<ToolResult, FunctionCallError> {
    let source = self
      .catalog
      .inspect(&call.tool_id)
      .map(|tool| tool.source)
      .unwrap_or(ToolSource::Builtin);

    let output = self
      .router
      .dispatch_tool_call(
        crate::tools::router::ToolCall {
          tool_name: call.tool_id.clone(),
          call_id: call.call_id.clone(),
          args: call.input,
        },
        ctx,
      )
      .await?;

    Ok(normalize_tool_result(source, output))
  }
}

fn normalize_tool_result(source: ToolSource, output: ToolOutput) -> ToolResult {
  match output {
    ToolOutput::Function { body, success, .. } => {
      let text = body.to_text();
      let content = serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text.clone()));
      let ok = success.unwrap_or(true);
      ToolResult {
        ok,
        error: (!ok).then_some(text.clone()),
        content,
        metadata: ToolResultMetadata {
          source,
          ..ToolResultMetadata::default()
        },
      }
    }
    ToolOutput::Mcp { result, .. } => match result {
      Ok(result) => ToolResult {
        ok: !result.is_error,
        error: result
          .is_error
          .then(|| serde_json::to_string(&result).unwrap_or_else(|_| "mcp error".to_string())),
        content: serde_json::to_value(&result).unwrap_or_else(|_| Value::Null),
        metadata: ToolResultMetadata {
          source,
          ..ToolResultMetadata::default()
        },
      },
      Err(error) => ToolResult {
        ok: false,
        content: Value::String(error.clone()),
        error: Some(error),
        metadata: ToolResultMetadata {
          source,
          ..ToolResultMetadata::default()
        },
      },
    },
  }
}

fn build_search_text(tool: &ToolDefinition) -> String {
  let mut parts = vec![
    tool.id.clone(),
    tool.name.clone(),
    tool.description.clone(),
    format!("{:?}", tool.source).to_lowercase(),
  ];
  parts.extend(tool.aliases.iter().cloned());
  parts.extend(tool.tags.iter().cloned());
  parts.extend(tool.input_keys.iter().cloned());
  if let Some(provider_id) = &tool.provider_id {
    parts.push(provider_id.clone());
  }
  if let Some(source_kind) = &tool.source_kind {
    parts.push(source_kind.clone());
  }
  if let Some(server_name) = &tool.server_name {
    parts.push(server_name.clone());
  }
  if let Some(remote_name) = &tool.remote_name {
    parts.push(remote_name.clone());
  }
  parts.join(" ")
}

fn tokenize(text: &str) -> Vec<String> {
  let mut tokens = Vec::new();
  let mut current = String::new();
  for ch in text.chars().flat_map(|ch| ch.to_lowercase()) {
    if ch.is_ascii_alphanumeric() {
      current.push(ch);
    } else if !current.is_empty() {
      tokens.push(std::mem::take(&mut current));
    }
  }
  if !current.is_empty() {
    tokens.push(current);
  }
  tokens
}

fn bm25_score(
  query_terms: &HashSet<String>,
  search_terms: &HashMap<String, usize>,
  doc_len: usize,
  document_frequency: &HashMap<String, usize>,
  active_count: usize,
  average_doc_len: f64,
) -> f64 {
  let k1 = 1.2;
  let b = 0.75;
  let doc_len = doc_len.max(1) as f64;
  query_terms
    .iter()
    .filter_map(|term| {
      let tf = *search_terms.get(term)? as f64;
      let df = *document_frequency.get(term).unwrap_or(&0) as f64;
      let n = active_count.max(1) as f64;
      let idf = (((n - df + 0.5) / (df + 0.5)) + 1.0).ln();
      let denom = tf + k1 * (1.0 - b + b * (doc_len / average_doc_len.max(1.0)));
      Some(idf * ((tf * (k1 + 1.0)) / denom))
    })
    .sum()
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::tool_runtime::ApprovalMode;
  use crate::tool_runtime::ToolApproval;
  use crate::tool_runtime::ToolRiskLevel;

  fn definition(name: &str, alias: Option<&str>) -> ToolDefinition {
    ToolDefinition {
      id: name.to_string(),
      name: name.to_string(),
      description: format!("tool {name}"),
      input_schema: serde_json::json!({"type":"object","properties":{}}),
      output_schema: None,
      source: ToolSource::Builtin,
      aliases: alias.into_iter().map(|value| value.to_string()).collect(),
      tags: vec!["builtin".to_string()],
      approval: ToolApproval {
        risk_level: ToolRiskLevel::Low,
        approval_mode: ApprovalMode::Auto,
        permission_key: Some(name.to_string()),
        allow_network: false,
        allow_fs_write: false,
      },
      enabled: true,
      supports_parallel: true,
      mutates_state: false,
      input_keys: vec!["path".to_string()],
      provider_id: Some("builtin".to_string()),
      source_kind: Some("builtin_primitive".to_string()),
      server_name: None,
      remote_name: None,
    }
  }

  #[test]
  fn inspect_matches_aliases() {
    let catalog =
      ToolRuntimeCatalog::from_tools(vec![definition("unified_exec", Some("container.exec"))]);
    let inspected = catalog.inspect("container.exec").expect("inspect alias");
    assert_eq!(inspected.id, "unified_exec");
  }

  #[test]
  fn search_prefers_matching_alias_terms() {
    let catalog = ToolRuntimeCatalog::from_tools(vec![
      definition("inspect_tool", None),
      definition("unified_exec", Some("container.exec")),
    ]);
    let matches = catalog.search("container exec", 5);
    assert_eq!(matches[0].tool.id, "unified_exec");
  }
}
