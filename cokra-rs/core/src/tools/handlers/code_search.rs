use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct CodeSearchHandler;

const EXA_MCP_URL: &str = "https://mcp.exa.ai/mcp";
const DEFAULT_LIMIT: usize = 10;
const DEFAULT_MATCHES: usize = 8;
const DEFAULT_SCOPE: &str = "local";

fn default_limit() -> usize {
  DEFAULT_LIMIT
}

fn default_matches() -> usize {
  DEFAULT_MATCHES
}

fn default_scope() -> String {
  DEFAULT_SCOPE.to_string()
}

#[derive(Debug, Deserialize)]
struct CodeSearchArgs {
  query: String,
  path: Option<String>,
  #[serde(default = "default_limit")]
  limit: usize,
  #[serde(default = "default_matches")]
  max_matches_per_file: usize,
  #[serde(default = "default_scope")]
  scope: String,
}

#[derive(Debug, Serialize)]
struct CodeSearchResponse {
  query: String,
  scope: String,
  backend: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  root: Option<String>,
  truncated: bool,
  items: Vec<CodeSearchItem>,
}

#[derive(Debug, Serialize)]
struct CodeSearchItem {
  kind: String,
  location: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  title: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  score: Option<f64>,
  #[serde(default)]
  matches: Vec<CodeSearchMatch>,
}

#[derive(Debug, Serialize)]
struct CodeSearchMatch {
  #[serde(default, skip_serializing_if = "Option::is_none")]
  line: Option<usize>,
  text: String,
}

#[derive(Debug, Deserialize)]
struct ExaResponse {
  result: Option<ExaResult>,
  error: Option<ExaError>,
}

#[derive(Debug, Deserialize)]
struct ExaResult {
  content: Vec<ExaContent>,
}

#[derive(Debug, Deserialize)]
struct ExaContent {
  #[serde(rename = "type")]
  kind: String,
  text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExaError {
  message: String,
}

#[async_trait]
impl ToolHandler for CodeSearchHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let args: CodeSearchArgs = invocation.parse_arguments()?;
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
    if args.max_matches_per_file == 0 {
      return Err(FunctionCallError::RespondToModel(
        "max_matches_per_file must be greater than zero".to_string(),
      ));
    }

    let scope = args.scope.trim().to_ascii_lowercase();
    let response = match scope.as_str() {
      "local" => search_local(&invocation, &args, query).await?,
      "web" => search_web(&invocation, &args, query).await?,
      _ => {
        return Err(FunctionCallError::RespondToModel(
          "scope must be one of: local, web".to_string(),
        ));
      }
    };

    let content = serde_json::to_string(&response).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize code_search result: {err}"))
    })?;
    Ok(ToolOutput::success(content).with_id(invocation.id))
  }
}

async fn search_local(
  invocation: &ToolInvocation,
  args: &CodeSearchArgs,
  query: &str,
) -> Result<CodeSearchResponse, FunctionCallError> {
  let root = invocation.resolve_path(args.path.as_deref());
  let params = cokra_file_search::SearchParams {
    root,
    query: query.to_string(),
    max_scanned_files: 1500,
    max_hits: args.limit.min(50),
    max_matches_per_file: args.max_matches_per_file.min(50),
    max_file_bytes: 256 * 1024,
  };

  let output = tokio::task::spawn_blocking(move || cokra_file_search::search(params))
    .await
    .map_err(|err| FunctionCallError::Execution(format!("code_search failed: {err}")))?
    .map_err(|err| FunctionCallError::Execution(format!("code_search failed: {err:#}")))?;

  Ok(CodeSearchResponse {
    query: output.query,
    scope: "local".to_string(),
    backend: "local_lexical".to_string(),
    root: Some(output.root.display().to_string()),
    truncated: output.truncated,
    items: output
      .hits
      .into_iter()
      .map(|hit| CodeSearchItem {
        kind: "file".to_string(),
        location: hit.path.display().to_string(),
        title: None,
        score: Some(hit.score as f64),
        matches: hit
          .matches
          .into_iter()
          .map(|item| CodeSearchMatch {
            line: Some(item.line),
            text: item.text,
          })
          .collect(),
      })
      .collect(),
  })
}

async fn search_web(
  invocation: &ToolInvocation,
  args: &CodeSearchArgs,
  query: &str,
) -> Result<CodeSearchResponse, FunctionCallError> {
  if let Some(runtime) = invocation.runtime.as_ref() {
    crate::tools::network_approval::authorize_http_url(
      runtime.as_ref(),
      &invocation.cwd,
      EXA_MCP_URL,
      &["mcp.exa.ai"],
    )
    .await
    .map_err(FunctionCallError::RespondToModel)?;
  }

  let client = reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(25))
    .user_agent("Mozilla/5.0 (compatible; cokra/1.0)")
    .build()
    .map_err(|err| FunctionCallError::Execution(format!("failed to build HTTP client: {err}")))?;

  let request = serde_json::json!({
    "jsonrpc": "2.0",
    "id": 1,
    "method": "tools/call",
    "params": {
      "name": "get_code_context_exa",
      "arguments": {
        "query": query,
        "tokensNum": (args.limit.min(20) * 800) as u32
      }
    }
  });

  let response = client
    .post(EXA_MCP_URL)
    .header("accept", "application/json, text/event-stream")
    .header("content-type", "application/json")
    .json(&request)
    .send()
    .await
    .map_err(|err| {
      FunctionCallError::RespondToModel(format!("remote code search request failed: {err}"))
    })?;

  if !response.status().is_success() {
    return Err(FunctionCallError::RespondToModel(format!(
      "remote code search failed with status {}",
      response.status()
    )));
  }

  let body = response.text().await.map_err(|err| {
    FunctionCallError::Execution(format!("failed to read remote code search response: {err}"))
  })?;
  let snippet = extract_remote_search_text(&body)?;

  Ok(CodeSearchResponse {
    query: query.to_string(),
    scope: "web".to_string(),
    backend: "exa_code_context".to_string(),
    root: None,
    truncated: false,
    items: vec![CodeSearchItem {
      kind: "web_document".to_string(),
      location: "exa_code_context".to_string(),
      title: Some(query.to_string()),
      score: None,
      matches: vec![CodeSearchMatch {
        line: None,
        text: snippet,
      }],
    }],
  })
}

fn extract_remote_search_text(body: &str) -> Result<String, FunctionCallError> {
  for line in body.lines() {
    if let Some(payload) = line.strip_prefix("data: ")
      && let Ok(response) = serde_json::from_str::<ExaResponse>(payload)
    {
      return exa_text(response);
    }
  }

  if let Ok(response) = serde_json::from_str::<ExaResponse>(body) {
    return exa_text(response);
  }

  Err(FunctionCallError::RespondToModel(
    "unexpected remote code search response format".to_string(),
  ))
}

fn exa_text(response: ExaResponse) -> Result<String, FunctionCallError> {
  if let Some(error) = response.error {
    return Err(FunctionCallError::RespondToModel(format!(
      "remote code search error: {}",
      error.message
    )));
  }
  Ok(
    response
      .result
      .map(|result| {
        result
          .content
          .into_iter()
          .filter(|item| item.kind == "text")
          .filter_map(|item| item.text)
          .collect::<Vec<_>>()
      })
      .unwrap_or_default()
      .into_iter()
      .next()
      .unwrap_or_else(|| "No remote code search results found".to_string()),
  )
}

#[cfg(test)]
mod tests {
  use super::*;
  use pretty_assertions::assert_eq;
  use tempfile::tempdir;

  #[tokio::test]
  async fn returns_ranked_local_items() -> anyhow::Result<()> {
    let temp = tempdir()?;
    std::fs::write(temp.path().join("a.rs"), "pub struct ToolRegistry {}\n")?;
    std::fs::write(temp.path().join("b.rs"), "fn other() {}\n")?;

    let inv = ToolInvocation {
      id: "1".to_string(),
      name: "code_search".to_string(),
      payload: crate::tools::context::ToolPayload::Function {
        arguments: serde_json::json!({
          "query": "ToolRegistry",
          "path": temp.path().display().to_string(),
          "limit": 5,
          "max_matches_per_file": 3,
          "scope": "local"
        })
        .to_string(),
      },
      cwd: temp.path().to_path_buf(),
      runtime: None,
    };

    let out = CodeSearchHandler.handle_async(inv).await?;
    let parsed: serde_json::Value = serde_json::from_str(&out.text_content())?;
    assert_eq!(parsed["query"], "ToolRegistry");
    assert_eq!(parsed["scope"], "local");
    assert!(
      parsed["items"]
        .as_array()
        .is_some_and(|items| !items.is_empty())
    );
    Ok(())
  }

  #[tokio::test]
  async fn rejects_unknown_scope() {
    let inv = ToolInvocation {
      id: "1".to_string(),
      name: "code_search".to_string(),
      payload: crate::tools::context::ToolPayload::Function {
        arguments: serde_json::json!({
          "query": "ToolRegistry",
          "scope": "invalid"
        })
        .to_string(),
      },
      cwd: std::env::temp_dir(),
      runtime: None,
    };

    let err = CodeSearchHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("scope"));
  }
}
