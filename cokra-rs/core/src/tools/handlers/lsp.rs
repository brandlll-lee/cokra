use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;

use crate::lsp;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct LspHandler;
pub struct LspStatusHandler;
pub struct LspRestartHandler;

const DEFAULT_MAX_RESULTS: usize = 20;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum LspOperation {
  GoToDefinition,
  FindReferences,
  Hover,
  DocumentSymbol,
  WorkspaceSymbol,
  GoToImplementation,
  PrepareCallHierarchy,
  IncomingCalls,
  OutgoingCalls,
}

impl LspOperation {
  fn as_str(self) -> &'static str {
    match self {
      Self::GoToDefinition => "goToDefinition",
      Self::FindReferences => "findReferences",
      Self::Hover => "hover",
      Self::DocumentSymbol => "documentSymbol",
      Self::WorkspaceSymbol => "workspaceSymbol",
      Self::GoToImplementation => "goToImplementation",
      Self::PrepareCallHierarchy => "prepareCallHierarchy",
      Self::IncomingCalls => "incomingCalls",
      Self::OutgoingCalls => "outgoingCalls",
    }
  }
}

#[derive(Debug, Deserialize)]
struct LspArgs {
  operation: LspOperation,
  #[serde(default)]
  file_path: Option<String>,
  #[serde(default)]
  line: Option<u32>,
  #[serde(default)]
  character: Option<u32>,
  #[serde(default)]
  query: Option<String>,
  #[serde(default = "default_max_results")]
  max_results: usize,
  #[serde(default)]
  symbol_kinds: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct LspRestartArgs {
  #[serde(default)]
  file_path: Option<String>,
  #[serde(default)]
  server_id: Option<String>,
}

fn default_max_results() -> usize {
  DEFAULT_MAX_RESULTS
}

#[async_trait]
impl ToolHandler for LspHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn is_mutating(&self, _: &ToolInvocation) -> bool {
    false
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let id = invocation.id.clone();
    let args: LspArgs = invocation.parse_arguments()?;
    let file_path = require_file_path(&invocation, args.file_path.as_deref(), args.operation)?;
    let result = run_lsp_operation(args.operation, &file_path, &args).await?;
    let result = apply_symbol_filters(result, &args.symbol_kinds, args.max_results);
    let result = normalize_result_paths(result, Some(&file_path));
    let summary = format_result_summary(args.operation, &file_path, result_count(&result));
    let payload = serde_json::json!({
      "operation": args.operation.as_str(),
      "file_path": file_path.display().to_string(),
      "result": result
    });
    let body = serde_json::to_string_pretty(&payload)
      .map_err(|err| FunctionCallError::Execution(format!("failed to encode LSP result: {err}")))?;
    Ok(ToolOutput::success(format!("{summary}\n\n{body}")).with_id(id))
  }
}

#[async_trait]
impl ToolHandler for LspStatusHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn is_mutating(&self, _: &ToolInvocation) -> bool {
    false
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let status = lsp::manager().status().await;
    let connected = status
      .clients
      .iter()
      .filter(|client| client.status == "connected")
      .count();
    let broken = status
      .clients
      .iter()
      .filter(|client| client.status == "broken")
      .count();
    let body = serde_json::to_string_pretty(&status)
      .map_err(|err| FunctionCallError::Execution(format!("failed to encode LSP status: {err}")))?;
    Ok(
      ToolOutput::success(format!(
        "LSP status: {connected} connected client(s), {broken} broken client(s)\n\n{body}"
      ))
      .with_id(invocation.id),
    )
  }
}

#[async_trait]
impl ToolHandler for LspRestartHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  fn is_mutating(&self, _: &ToolInvocation) -> bool {
    true
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: LspRestartArgs = invocation.parse_arguments()?;
    let file_path = args
      .file_path
      .as_deref()
      .map(|path| invocation.resolve_path(Some(path)));
    let report = lsp::manager()
      .restart(file_path.as_deref(), args.server_id.as_deref())
      .await
      .map_err(map_lsp_error)?;
    let body = serde_json::to_string_pretty(&report).map_err(|err| {
      FunctionCallError::Execution(format!("failed to encode LSP restart report: {err}"))
    })?;
    Ok(
      ToolOutput::success(format!(
        "Restarted {} client(s) and cleared {} broken entries\n\n{}",
        report.restarted_clients, report.cleared_broken_entries, body
      ))
      .with_id(invocation.id),
    )
  }
}

async fn run_lsp_operation(
  operation: LspOperation,
  file_path: &Path,
  args: &LspArgs,
) -> Result<Value, FunctionCallError> {
  let manager = lsp::manager();
  match operation {
    LspOperation::GoToDefinition => {
      let (line, character) = require_position(args)?;
      manager
        .text_document_request(
          file_path,
          "textDocument/definition",
          serde_json::json!({
            "textDocument": { "uri": lsp::path_to_uri(file_path) },
            "position": { "line": line, "character": character }
          }),
          true,
        )
        .await
        .map_err(map_lsp_error)
    }
    LspOperation::FindReferences => {
      let (line, character) = require_position(args)?;
      manager
        .text_document_request(
          file_path,
          "textDocument/references",
          serde_json::json!({
            "textDocument": { "uri": lsp::path_to_uri(file_path) },
            "position": { "line": line, "character": character },
            "context": { "includeDeclaration": true }
          }),
          true,
        )
        .await
        .map_err(map_lsp_error)
    }
    LspOperation::Hover => {
      let (line, character) = require_position(args)?;
      manager
        .text_document_request(
          file_path,
          "textDocument/hover",
          serde_json::json!({
            "textDocument": { "uri": lsp::path_to_uri(file_path) },
            "position": { "line": line, "character": character }
          }),
          true,
        )
        .await
        .map_err(map_lsp_error)
    }
    LspOperation::DocumentSymbol => manager
      .text_document_request(
        file_path,
        "textDocument/documentSymbol",
        serde_json::json!({
          "textDocument": { "uri": lsp::path_to_uri(file_path) }
        }),
        true,
      )
      .await
      .map_err(map_lsp_error),
    LspOperation::WorkspaceSymbol => manager
      .workspace_request(
        file_path,
        "workspace/symbol",
        serde_json::json!({
          "query": args.query.clone().unwrap_or_default()
        }),
      )
      .await
      .map_err(map_lsp_error),
    LspOperation::GoToImplementation => {
      let (line, character) = require_position(args)?;
      manager
        .text_document_request(
          file_path,
          "textDocument/implementation",
          serde_json::json!({
            "textDocument": { "uri": lsp::path_to_uri(file_path) },
            "position": { "line": line, "character": character }
          }),
          true,
        )
        .await
        .map_err(map_lsp_error)
    }
    LspOperation::PrepareCallHierarchy => {
      let (line, character) = require_position(args)?;
      manager
        .text_document_request(
          file_path,
          "textDocument/prepareCallHierarchy",
          serde_json::json!({
            "textDocument": { "uri": lsp::path_to_uri(file_path) },
            "position": { "line": line, "character": character }
          }),
          true,
        )
        .await
        .map_err(map_lsp_error)
    }
    LspOperation::IncomingCalls => {
      let item = prepare_call_hierarchy(file_path, args).await?;
      if item.is_null() {
        return Ok(Value::Array(Vec::new()));
      }
      manager
        .text_document_request(
          file_path,
          "callHierarchy/incomingCalls",
          serde_json::json!({ "item": item }),
          true,
        )
        .await
        .map_err(map_lsp_error)
    }
    LspOperation::OutgoingCalls => {
      let item = prepare_call_hierarchy(file_path, args).await?;
      if item.is_null() {
        return Ok(Value::Array(Vec::new()));
      }
      manager
        .text_document_request(
          file_path,
          "callHierarchy/outgoingCalls",
          serde_json::json!({ "item": item }),
          true,
        )
        .await
        .map_err(map_lsp_error)
    }
  }
}

async fn prepare_call_hierarchy(
  file_path: &Path,
  args: &LspArgs,
) -> Result<Value, FunctionCallError> {
  let (line, character) = require_position(args)?;
  let prepared = lsp::manager()
    .text_document_request(
      file_path,
      "textDocument/prepareCallHierarchy",
      serde_json::json!({
        "textDocument": { "uri": lsp::path_to_uri(file_path) },
        "position": { "line": line, "character": character }
      }),
      true,
    )
    .await
    .map_err(map_lsp_error)?;

  match prepared {
    Value::Array(items) => Ok(items.into_iter().next().unwrap_or(Value::Null)),
    other => Ok(other),
  }
}

fn require_file_path(
  invocation: &ToolInvocation,
  file_path: Option<&str>,
  operation: LspOperation,
) -> Result<PathBuf, FunctionCallError> {
  let file_path = file_path.ok_or_else(|| {
    FunctionCallError::RespondToModel(format!("{} requires file_path", operation.as_str()))
  })?;
  Ok(invocation.resolve_path(Some(file_path)))
}

fn require_position(args: &LspArgs) -> Result<(u32, u32), FunctionCallError> {
  let line = args.line.ok_or_else(|| {
    FunctionCallError::RespondToModel("line is required for this LSP operation".to_string())
  })?;
  let character = args.character.ok_or_else(|| {
    FunctionCallError::RespondToModel("character is required for this LSP operation".to_string())
  })?;
  if line == 0 || character == 0 {
    return Err(FunctionCallError::RespondToModel(
      "line and character must be 1-based positive integers".to_string(),
    ));
  }
  Ok((line - 1, character - 1))
}

fn format_result_summary(operation: LspOperation, file_path: &Path, count: usize) -> String {
  if count == 0 {
    return format!(
      "LSP {} returned no results for {}",
      operation.as_str(),
      file_path.display()
    );
  }
  format!(
    "LSP {} returned {} result(s) for {}",
    operation.as_str(),
    count,
    file_path.display()
  )
}

fn result_count(value: &Value) -> usize {
  match value {
    Value::Null => 0,
    Value::Array(items) => items.len(),
    _ => 1,
  }
}

fn apply_symbol_filters(value: Value, symbol_kinds: &[String], max_results: usize) -> Value {
  let allowed = parse_symbol_kinds(symbol_kinds);
  let value = if allowed.is_empty() {
    value
  } else {
    filter_symbol_items(value, &allowed).unwrap_or(Value::Array(Vec::new()))
  };
  let mut remaining = max_results.max(1);
  limit_symbol_items(value, &mut remaining).unwrap_or(Value::Array(Vec::new()))
}

fn parse_symbol_kinds(symbol_kinds: &[String]) -> HashSet<u64> {
  symbol_kinds
    .iter()
    .filter_map(|kind| match kind.to_ascii_lowercase().as_str() {
      "file" => Some(1),
      "module" => Some(2),
      "namespace" => Some(3),
      "package" => Some(4),
      "class" => Some(5),
      "method" => Some(6),
      "property" => Some(7),
      "field" => Some(8),
      "constructor" => Some(9),
      "enum" => Some(10),
      "interface" => Some(11),
      "function" => Some(12),
      "variable" => Some(13),
      "constant" => Some(14),
      "string" => Some(15),
      "number" => Some(16),
      "boolean" => Some(17),
      "array" => Some(18),
      "object" => Some(19),
      "key" => Some(20),
      "null" => Some(21),
      "enum_member" => Some(22),
      "struct" => Some(23),
      "event" => Some(24),
      "operator" => Some(25),
      "type_parameter" => Some(26),
      other => other.parse::<u64>().ok(),
    })
    .collect()
}

fn filter_symbol_items(value: Value, allowed: &HashSet<u64>) -> Option<Value> {
  match value {
    Value::Array(items) => Some(Value::Array(
      items
        .into_iter()
        .filter_map(|item| filter_symbol_items(item, allowed))
        .collect(),
    )),
    Value::Object(mut map) => {
      if let Some(kind) = map.get("kind").and_then(Value::as_u64)
        && !allowed.contains(&kind)
      {
        return None;
      }
      if let Some(children) = map.remove("children")
        && let Some(filtered) = filter_symbol_items(children, allowed)
      {
        map.insert("children".to_string(), filtered);
      }
      Some(Value::Object(map))
    }
    other => Some(other),
  }
}

fn limit_symbol_items(value: Value, remaining: &mut usize) -> Option<Value> {
  match value {
    Value::Array(items) => Some(Value::Array(
      items
        .into_iter()
        .filter_map(|item| {
          if *remaining == 0 {
            return None;
          }
          limit_symbol_items(item, remaining)
        })
        .collect(),
    )),
    Value::Object(mut map) => {
      if map.contains_key("kind") {
        if *remaining == 0 {
          return None;
        }
        *remaining -= 1;
      }
      if let Some(children) = map.remove("children")
        && let Some(limited) = limit_symbol_items(children, remaining)
      {
        map.insert("children".to_string(), limited);
      }
      Some(Value::Object(map))
    }
    other => Some(other),
  }
}

fn normalize_result_paths(value: Value, fallback_path: Option<&Path>) -> Value {
  match value {
    Value::Array(items) => Value::Array(
      items
        .into_iter()
        .map(|item| normalize_result_paths(item, fallback_path))
        .collect(),
    ),
    Value::Object(map) => {
      let mut normalized = Map::new();
      for (key, value) in map {
        if key == "uri" {
          normalized.insert(key.clone(), value.clone());
          if let Some(uri) = value.as_str()
            && let Some(path) = lsp::uri_to_path(uri)
          {
            normalized.insert(
              "path".to_string(),
              Value::String(path.display().to_string()),
            );
          }
          continue;
        }
        if key.ends_with("Uri") {
          normalized.insert(key.clone(), value.clone());
          if let Some(uri) = value.as_str()
            && let Some(path) = lsp::uri_to_path(uri)
          {
            let stem = key.trim_end_matches("Uri");
            normalized.insert(
              format!("{stem}Path"),
              Value::String(path.display().to_string()),
            );
          }
          continue;
        }
        normalized.insert(key, normalize_result_paths(value, fallback_path));
      }
      if !normalized.contains_key("path")
        && let Some(path) = fallback_path
        && normalized.contains_key("kind")
      {
        normalized.insert(
          "path".to_string(),
          Value::String(path.display().to_string()),
        );
      }
      Value::Object(normalized)
    }
    other => other,
  }
}

fn map_lsp_error(error: lsp::LspError) -> FunctionCallError {
  match error {
    lsp::LspError::Disabled
    | lsp::LspError::FileNotFound(_)
    | lsp::LspError::UnsupportedFile(_)
    | lsp::LspError::ServerUnavailable(_) => FunctionCallError::RespondToModel(error.to_string()),
    lsp::LspError::RequestFailed(_) => FunctionCallError::Execution(error.to_string()),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_symbol_kinds_from_names() {
    let kinds = parse_symbol_kinds(&["function".to_string(), "class".to_string()]);
    assert!(kinds.contains(&12));
    assert!(kinds.contains(&5));
  }

  #[test]
  fn filter_symbol_items_removes_unwanted_kinds() {
    let filtered = filter_symbol_items(
      serde_json::json!([
        { "name": "keep", "kind": 12 },
        { "name": "drop", "kind": 5 }
      ]),
      &HashSet::from([12]),
    )
    .expect("filtered");
    assert_eq!(
      filtered,
      serde_json::json!([{ "name": "keep", "kind": 12 }])
    );
  }

  #[test]
  fn normalize_result_paths_adds_path_for_file_uris() {
    let normalized = normalize_result_paths(
      serde_json::json!({
        "uri": "file:///tmp/demo.rs",
        "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }
      }),
      None,
    );
    assert!(normalized.get("path").is_some());
  }
}
