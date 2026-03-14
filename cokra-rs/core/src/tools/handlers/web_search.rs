//! web_search tool handler — multi-backend web search.
//!
//! Supports three backends, selected via config or env:
//!   1. **Exa** (default, no key required for public endpoint) — MCP JSON-RPC over HTTP
//!   2. **Brave Search API** — requires BRAVE_SEARCH_API_KEY
//!   3. **SearXNG** — self-hosted, requires SEARXNG_BASE_URL
//!
//! Backend selection order:
//!   1. `BRAVE_SEARCH_API_KEY` env var → Brave
//!   2. `SEARXNG_BASE_URL` env var → SearXNG
//!   3. Fallback → Exa public endpoint

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct WebSearchHandler;

const DEFAULT_NUM_RESULTS: u32 = 8;
const SEARCH_TIMEOUT_SECS: u64 = 25;
const MAX_CONTENT_CHARS: usize = 20_000;

// ── Exa MCP endpoint (1:1 opencode websearch.ts) ────────────────────────────

const EXA_MCP_URL: &str = "https://mcp.exa.ai/mcp";

#[derive(Debug, Serialize)]
struct ExaMcpRequest {
  jsonrpc: &'static str,
  id: u32,
  method: &'static str,
  params: ExaMcpParams,
}

#[derive(Debug, Serialize)]
struct ExaMcpParams {
  name: &'static str,
  arguments: ExaSearchArgs,
}

#[derive(Debug, Serialize)]
struct ExaSearchArgs {
  query: String,
  #[serde(rename = "numResults")]
  num_results: u32,
  #[serde(skip_serializing_if = "Option::is_none")]
  livecrawl: Option<String>,
  #[serde(
    rename = "contextMaxCharacters",
    skip_serializing_if = "Option::is_none"
  )]
  context_max_characters: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ExaMcpResponse {
  result: Option<ExaMcpResult>,
  error: Option<ExaMcpError>,
}

#[derive(Debug, Deserialize)]
struct ExaMcpResult {
  content: Vec<ExaContentItem>,
}

#[derive(Debug, Deserialize)]
struct ExaContentItem {
  #[serde(rename = "type")]
  kind: String,
  text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExaMcpError {
  message: String,
}

// ── Brave Search API ─────────────────────────────────────────────────────────

const BRAVE_SEARCH_URL: &str = "https://api.search.brave.com/res/v1/web/search";

#[derive(Debug, Deserialize)]
struct BraveSearchResponse {
  web: Option<BraveWebResults>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResults {
  results: Vec<BraveResult>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
  title: String,
  url: String,
  description: Option<String>,
}

// ── SearXNG ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SearxngResponse {
  results: Vec<SearxngResult>,
}

#[derive(Debug, Deserialize)]
struct SearxngResult {
  title: String,
  url: String,
  content: Option<String>,
}

// ── Tool args ────────────────────────────────────────────────────────────────

fn default_num_results() -> u32 {
  DEFAULT_NUM_RESULTS
}

fn default_livecrawl() -> String {
  "fallback".to_string()
}

#[derive(Debug, Deserialize)]
struct WebSearchArgs {
  query: String,
  #[serde(default = "default_num_results")]
  num_results: u32,
  /// livecrawl mode: "fallback" | "preferred"
  #[serde(default = "default_livecrawl")]
  livecrawl: String,
  /// context_max_characters for Exa backend
  #[serde(default)]
  context_max_characters: Option<u32>,
}

// ── Handler ──────────────────────────────────────────────────────────────────

#[async_trait]
impl ToolHandler for WebSearchHandler {
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
    let args: WebSearchArgs = invocation.parse_arguments()?;

    if args.query.trim().is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "query must not be empty".to_string(),
      ));
    }

    let client = reqwest::Client::builder()
      .timeout(std::time::Duration::from_secs(SEARCH_TIMEOUT_SECS))
      .user_agent("Mozilla/5.0 (compatible; cokra/1.0)")
      .build()
      .map_err(|e| FunctionCallError::Execution(format!("failed to build HTTP client: {e}")))?;

    let output = if let Ok(api_key) = std::env::var("BRAVE_SEARCH_API_KEY") {
      if let Some(runtime) = invocation.runtime.as_ref() {
        crate::tools::network_approval::authorize_http_url(
          runtime.as_ref(),
          &invocation.cwd,
          BRAVE_SEARCH_URL,
          &["api.search.brave.com"],
        )
        .await
        .map_err(FunctionCallError::RespondToModel)?;
      }
      search_brave(&client, &args, &api_key).await?
    } else if let Ok(base_url) = std::env::var("SEARXNG_BASE_URL") {
      let search_url = format!("{}/search", base_url.trim_end_matches('/'));
      if let Some(runtime) = invocation.runtime.as_ref() {
        crate::tools::network_approval::authorize_http_url(
          runtime.as_ref(),
          &invocation.cwd,
          &search_url,
          &[],
        )
        .await
        .map_err(FunctionCallError::RespondToModel)?;
      }
      search_searxng(&client, &args, &base_url).await?
    } else {
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
      search_exa(&client, &args).await?
    };

    Ok(ToolOutput::success(output).with_id(id))
  }
}

// ── Exa backend ──────────────────────────────────────────────────────────────

async fn search_exa(
  client: &reqwest::Client,
  args: &WebSearchArgs,
) -> Result<String, FunctionCallError> {
  let request = ExaMcpRequest {
    jsonrpc: "2.0",
    id: 1,
    method: "tools/call",
    params: ExaMcpParams {
      name: "web_search_exa",
      arguments: ExaSearchArgs {
        query: args.query.clone(),
        num_results: args.num_results,
        livecrawl: Some(args.livecrawl.clone()),
        context_max_characters: args.context_max_characters,
      },
    },
  };

  let response = client
    .post(EXA_MCP_URL)
    .header("accept", "application/json, text/event-stream")
    .header("content-type", "application/json")
    .json(&request)
    .send()
    .await
    .map_err(|e| FunctionCallError::RespondToModel(format!("Exa search request failed: {e}")))?;

  if !response.status().is_success() {
    let status = response.status();
    return Err(FunctionCallError::RespondToModel(format!(
      "Exa search failed with status {status}"
    )));
  }

  let body = response
    .text()
    .await
    .map_err(|e| FunctionCallError::Execution(format!("failed to read Exa response: {e}")))?;

  // Parse SSE or plain JSON response (1:1 opencode pattern)
  let parsed = parse_exa_response(&body, &args.query)?;
  Ok(parsed)
}

fn parse_exa_response(body: &str, query: &str) -> Result<String, FunctionCallError> {
  // Try SSE format first: lines starting with "data: "
  for line in body.lines() {
    if let Some(json_str) = line.strip_prefix("data: ")
      && let Ok(resp) = serde_json::from_str::<ExaMcpResponse>(json_str)
    {
      return extract_exa_text(resp, query);
    }
  }

  // Try plain JSON
  if let Ok(resp) = serde_json::from_str::<ExaMcpResponse>(body) {
    return extract_exa_text(resp, query);
  }

  Err(FunctionCallError::RespondToModel(format!(
    "unexpected Exa response format for query \"{query}\""
  )))
}

fn extract_exa_text(resp: ExaMcpResponse, query: &str) -> Result<String, FunctionCallError> {
  if let Some(err) = resp.error {
    return Err(FunctionCallError::RespondToModel(format!(
      "Exa search error: {}",
      err.message
    )));
  }

  let result = resp
    .result
    .ok_or_else(|| FunctionCallError::RespondToModel(format!("no results for \"{query}\"")))?;

  let text = result
    .content
    .into_iter()
    .filter(|item| item.kind == "text")
    .filter_map(|item| item.text)
    .next()
    .unwrap_or_else(|| format!("No search results found for \"{query}\""));

  let truncated = truncate_to_chars(text, MAX_CONTENT_CHARS);
  Ok(format!(
    "Web search results for \"{query}\":\n\n{truncated}"
  ))
}

// ── Brave backend ─────────────────────────────────────────────────────────────

async fn search_brave(
  client: &reqwest::Client,
  args: &WebSearchArgs,
  api_key: &str,
) -> Result<String, FunctionCallError> {
  let response = client
    .get(BRAVE_SEARCH_URL)
    .header("Accept", "application/json")
    .header("Accept-Encoding", "gzip")
    .header("X-Subscription-Token", api_key)
    .query(&[
      ("q", args.query.as_str()),
      ("count", &args.num_results.to_string()),
    ])
    .send()
    .await
    .map_err(|e| FunctionCallError::RespondToModel(format!("Brave search request failed: {e}")))?;

  if !response.status().is_success() {
    let status = response.status();
    return Err(FunctionCallError::RespondToModel(format!(
      "Brave search failed with status {status}"
    )));
  }

  let resp: BraveSearchResponse = response
    .json()
    .await
    .map_err(|e| FunctionCallError::Execution(format!("failed to parse Brave response: {e}")))?;

  let results = resp.web.map(|w| w.results).unwrap_or_default();

  if results.is_empty() {
    return Ok(format!("No search results found for \"{}\"", args.query));
  }

  let mut output = format!("Web search results for \"{}\":\n\n", args.query);
  for (i, result) in results.iter().enumerate() {
    output.push_str(&format!("{}. **{}**\n", i + 1, result.title));
    output.push_str(&format!("   URL: {}\n", result.url));
    if let Some(desc) = &result.description {
      output.push_str(&format!("   {}\n", desc));
    }
    output.push('\n');
  }

  Ok(truncate_to_chars(output, MAX_CONTENT_CHARS))
}

// ── SearXNG backend ───────────────────────────────────────────────────────────

async fn search_searxng(
  client: &reqwest::Client,
  args: &WebSearchArgs,
  base_url: &str,
) -> Result<String, FunctionCallError> {
  let search_url = format!("{}/search", base_url.trim_end_matches('/'));

  let response = client
    .get(&search_url)
    .query(&[
      ("q", args.query.as_str()),
      ("format", "json"),
      ("engines", "google,bing,duckduckgo"),
    ])
    .send()
    .await
    .map_err(|e| {
      FunctionCallError::RespondToModel(format!("SearXNG search request failed: {e}"))
    })?;

  if !response.status().is_success() {
    let status = response.status();
    return Err(FunctionCallError::RespondToModel(format!(
      "SearXNG search failed with status {status}"
    )));
  }

  let resp: SearxngResponse = response
    .json()
    .await
    .map_err(|e| FunctionCallError::Execution(format!("failed to parse SearXNG response: {e}")))?;

  if resp.results.is_empty() {
    return Ok(format!("No search results found for \"{}\"", args.query));
  }

  let mut output = format!("Web search results for \"{}\":\n\n", args.query);
  let count = (args.num_results as usize).min(resp.results.len());
  for (i, result) in resp.results.iter().take(count).enumerate() {
    output.push_str(&format!("{}. **{}**\n", i + 1, result.title));
    output.push_str(&format!("   URL: {}\n", result.url));
    if let Some(content) = &result.content {
      output.push_str(&format!("   {}\n", content));
    }
    output.push('\n');
  }

  Ok(truncate_to_chars(output, MAX_CONTENT_CHARS))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn truncate_to_chars(s: String, max: usize) -> String {
  if s.len() <= max {
    return s;
  }
  // Find a valid UTF-8 boundary at or before `max` bytes.
  let boundary = (0..=max)
    .rev()
    .find(|&i| s.is_char_boundary(i))
    .unwrap_or(0);
  format!(
    "{}\n\n[Content truncated at {max} characters]",
    &s[..boundary]
  )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
  use super::*;
  use crate::tools::context::ToolPayload;

  fn make_inv(args: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
      id: "test-1".to_string(),
      name: "web_search".to_string(),
      payload: ToolPayload::Function {
        arguments: args.to_string(),
      },
      cwd: std::env::temp_dir(),
      runtime: None,
    }
  }

  #[tokio::test]
  async fn rejects_empty_query() {
    let inv = make_inv(serde_json::json!({ "query": "  " }));
    let err = WebSearchHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("empty"));
  }

  #[tokio::test]
  async fn rejects_missing_query() {
    let inv = make_inv(serde_json::json!({}));
    let err = WebSearchHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("query"));
  }

  #[test]
  fn parse_exa_sse_response() {
    let sse_body = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"Rust is a systems language.\"}]}}\n";
    let result = parse_exa_response(sse_body, "rust programming").unwrap();
    assert!(result.contains("Rust is a systems language."));
    assert!(result.contains("rust programming"));
  }

  #[test]
  fn parse_exa_plain_json_response() {
    let json_body =
      r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"Hello world"}]}}"#;
    let result = parse_exa_response(json_body, "hello").unwrap();
    assert!(result.contains("Hello world"));
  }

  #[test]
  fn parse_exa_error_response() {
    let json_body = r#"{"jsonrpc":"2.0","id":1,"error":{"message":"Rate limit exceeded"}}"#;
    let err = parse_exa_response(json_body, "test").unwrap_err();
    assert!(err.to_string().contains("Rate limit exceeded"));
  }

  #[test]
  fn truncate_to_chars_short() {
    let s = "hello".to_string();
    assert_eq!(truncate_to_chars(s.clone(), 100), s);
  }

  #[test]
  fn truncate_to_chars_long() {
    let s = "a".repeat(30_000);
    let result = truncate_to_chars(s, 20_000);
    assert!(result.contains("[Content truncated"));
  }

  #[test]
  fn default_num_results_is_8() {
    let args: WebSearchArgs = serde_json::from_str(r#"{"query":"test"}"#).unwrap();
    assert_eq!(args.num_results, 8);
  }

  #[test]
  fn default_livecrawl_is_fallback() {
    let args: WebSearchArgs = serde_json::from_str(r#"{"query":"test"}"#).unwrap();
    assert_eq!(args.livecrawl, "fallback");
  }

  #[test]
  fn custom_num_results_parsed() {
    let args: WebSearchArgs = serde_json::from_str(r#"{"query":"test","num_results":5}"#).unwrap();
    assert_eq!(args.num_results, 5);
  }
}
