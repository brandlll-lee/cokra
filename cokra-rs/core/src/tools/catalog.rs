use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use serde::Serialize;

use cokra_config::McpConfig;

use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::registry::ToolRegistry;
use crate::tools::spec::JsonSchema;
use crate::tools::spec::ToolHandlerType;
use crate::tools::spec::ToolPermissions;
use crate::tools::spec::ToolSourceKind;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCatalogSource {
  Builtin,
  Mcp,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCatalogEntry {
  pub canonical_name: String,
  pub aliases: Vec<String>,
  pub description: String,
  pub handler_type: ToolHandlerType,
  pub source_kind: ToolSourceKind,
  pub permissions: ToolPermissions,
  pub permission_key: Option<String>,
  pub input_schema: JsonSchema,
  pub input_keys: Vec<String>,
  pub source: ToolCatalogSource,
  pub is_active: bool,
  pub supports_parallel: bool,
  pub mutates_state: bool,
  pub is_mutating: bool,
  pub server_name: Option<String>,
  pub remote_tool_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCatalogMatch {
  pub entry: ToolCatalogEntry,
  pub score: f64,
  pub matched_terms: Vec<String>,
}

#[derive(Debug, Clone)]
struct IndexedEntry {
  entry: ToolCatalogEntry,
  search_terms: HashMap<String, usize>,
  doc_len: usize,
}

#[derive(Debug, Clone)]
pub struct ToolCatalog {
  entries: Vec<IndexedEntry>,
  by_name: HashMap<String, usize>,
  document_frequency: HashMap<String, usize>,
  active_count: usize,
  average_doc_len: f64,
}

impl ToolCatalog {
  pub fn from_registry(registry: &ToolRegistry, mcp_config: &McpConfig) -> Self {
    let mcp_sources = build_mcp_source_index(mcp_config);
    let mut entries = Vec::new();
    let mut by_name = HashMap::new();

    for spec in registry.list_specs() {
      let aliases = registry.aliases_for(&spec.name);
      let source = catalog_source_from_kind(&spec.source_kind);
      let (server_name, remote_tool_name) = match source {
        ToolCatalogSource::Builtin => (None, None),
        ToolCatalogSource::Mcp => mcp_sources
          .get(&spec.name)
          .cloned()
          .unwrap_or_else(|| parse_mcp_tool_name(&spec.name)),
      };
      let entry = ToolCatalogEntry {
        canonical_name: spec.name.clone(),
        aliases: aliases.clone(),
        description: spec.description.clone(),
        handler_type: spec.handler_type.clone(),
        source_kind: spec.source_kind.clone(),
        permissions: spec.permissions.clone(),
        permission_key: spec.permission_key.clone(),
        input_schema: spec.input_schema.clone(),
        input_keys: collect_input_keys(&spec.input_schema),
        source,
        is_active: !registry.is_excluded(&spec.name),
        supports_parallel: spec.supports_parallel,
        mutates_state: spec.mutates_state,
        is_mutating: spec.mutates_state || tool_is_mutating(registry, &spec.name),
        server_name,
        remote_tool_name,
      };
      let index = entries.len();
      for name in std::iter::once(entry.canonical_name.clone()).chain(aliases.into_iter()) {
        by_name.insert(name, index);
      }
      entries.push(IndexedEntry::new(entry));
    }

    let active_entries = entries
      .iter()
      .filter(|entry| entry.entry.is_active)
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

  pub fn inspect(&self, name: &str) -> Option<ToolCatalogEntry> {
    let key = name.trim();
    self
      .by_name
      .get(key)
      .and_then(|index| self.entries.get(*index))
      .map(|entry| entry.entry.clone())
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
      .filter(|entry| entry.entry.is_active)
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
          entry: entry.entry.clone(),
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
        .then_with(|| left.entry.canonical_name.cmp(&right.entry.canonical_name))
    });
    matches.truncate(limit);
    matches
  }
}

impl IndexedEntry {
  fn new(entry: ToolCatalogEntry) -> Self {
    let search_text = build_search_text(&entry);
    let tokens = tokenize(&search_text);
    let mut search_terms = HashMap::new();
    for token in tokens {
      *search_terms.entry(token).or_insert(0) += 1;
    }
    let doc_len = search_terms.values().sum();
    Self {
      entry,
      search_terms,
      doc_len,
    }
  }
}

fn build_search_text(entry: &ToolCatalogEntry) -> String {
  let mut parts = vec![
    entry.canonical_name.clone(),
    entry.description.clone(),
    format!("{:?}", entry.handler_type).to_lowercase(),
    format!("{:?}", entry.source_kind).to_lowercase(),
  ];
  parts.extend(entry.aliases.iter().cloned());
  parts.extend(entry.input_keys.iter().cloned());
  if let Some(permission_key) = &entry.permission_key {
    parts.push(permission_key.clone());
  }
  if let Some(server_name) = &entry.server_name {
    parts.push(server_name.clone());
  }
  if let Some(remote_tool_name) = &entry.remote_tool_name {
    parts.push(remote_tool_name.clone());
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

fn collect_input_keys(schema: &JsonSchema) -> Vec<String> {
  match schema {
    JsonSchema::Object { properties, .. } => properties.keys().cloned().collect(),
    _ => Vec::new(),
  }
}

fn catalog_source_from_kind(source_kind: &ToolSourceKind) -> ToolCatalogSource {
  match source_kind {
    ToolSourceKind::Mcp => ToolCatalogSource::Mcp,
    ToolSourceKind::BuiltinPrimitive
    | ToolSourceKind::BuiltinCollaboration
    | ToolSourceKind::BuiltinWorkflow => ToolCatalogSource::Builtin,
  }
}

fn tool_is_mutating(registry: &ToolRegistry, tool_name: &str) -> bool {
  let invocation = ToolInvocation {
    id: "catalog".to_string(),
    name: tool_name.to_string(),
    payload: ToolPayload::Function {
      arguments: "{}".to_string(),
    },
    cwd: PathBuf::from("."),
    runtime: None,
  };
  registry.is_mutating(&invocation).unwrap_or(false)
}

fn build_mcp_source_index(config: &McpConfig) -> HashMap<String, (Option<String>, Option<String>)> {
  let mut result = HashMap::new();
  for (server_name, server) in config.servers.iter().filter(|(_, server)| server.enabled) {
    let Some(enabled_tools) = &server.enabled_tools else {
      continue;
    };
    for tool_name in enabled_tools {
      let exposed = sanitize_tool_name(&format!("mcp__{server_name}__{tool_name}"));
      result.insert(
        exposed,
        (Some(server_name.clone()), Some(tool_name.clone())),
      );
    }
  }
  result
}

fn parse_mcp_tool_name(tool_name: &str) -> (Option<String>, Option<String>) {
  let Some(stripped) = tool_name.strip_prefix("mcp__") else {
    return (None, None);
  };
  let Some((server, tool)) = stripped.split_once("__") else {
    return (None, None);
  };
  if server.is_empty() || tool.is_empty() {
    return (None, None);
  }
  (Some(server.to_string()), Some(tool.to_string()))
}

fn sanitize_tool_name(name: &str) -> String {
  let sanitized = name
    .chars()
    .map(|ch| {
      if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
        ch
      } else {
        '_'
      }
    })
    .collect::<String>();
  if sanitized.is_empty() {
    "_".to_string()
  } else {
    sanitized
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::BTreeMap;
  use std::sync::Arc;

  use async_trait::async_trait;

  use crate::tools::context::FunctionCallError;
  use crate::tools::context::ToolOutput;
  use crate::tools::registry::ToolHandler;
  use crate::tools::registry::ToolKind;
  use crate::tools::spec::ToolSpec;

  struct DummyHandler {
    mutating: bool,
  }

  #[async_trait]
  impl ToolHandler for DummyHandler {
    fn kind(&self) -> ToolKind {
      ToolKind::Function
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
      self.mutating
    }

    fn handle(&self, _invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
      Ok(ToolOutput::success("ok"))
    }
  }

  fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec::new(
      name,
      description,
      JsonSchema::Object {
        properties: BTreeMap::from([
          ("path".to_string(), JsonSchema::String { description: None }),
          (
            "query".to_string(),
            JsonSchema::String { description: None },
          ),
        ]),
        required: Some(vec![]),
        additional_properties: Some(false.into()),
      },
      None,
      ToolHandlerType::Function,
      ToolPermissions::default(),
    )
  }

  #[test]
  fn search_matches_aliases_and_descriptions() {
    let mut registry = ToolRegistry::new();
    registry.register_tool(
      spec("unified_exec", "Run argv commands"),
      Arc::new(DummyHandler { mutating: true }),
    );
    registry.register_alias("container.exec", "unified_exec");

    let catalog = ToolCatalog::from_registry(&registry, &McpConfig::default());
    let results = catalog.search("container exec", 5);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].entry.canonical_name, "unified_exec");
    assert!(
      results[0]
        .entry
        .aliases
        .contains(&"container.exec".to_string())
    );
  }

  #[test]
  fn inspect_resolves_aliases() {
    let mut registry = ToolRegistry::new();
    registry.register_tool(
      spec("shell", "Run a shell command"),
      Arc::new(DummyHandler { mutating: true }),
    );
    registry.register_alias("exec", "shell");

    let catalog = ToolCatalog::from_registry(&registry, &McpConfig::default());
    let entry = catalog.inspect("exec").expect("alias should resolve");

    assert_eq!(entry.canonical_name, "shell");
    assert!(entry.aliases.contains(&"exec".to_string()));
  }
}
