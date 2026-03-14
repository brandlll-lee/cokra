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
const DEFAULT_CONTEXT_MAX_CHARACTERS: u32 = 10_000;
const MAX_RESULT_SNIPPET_CHARS_SHORT: usize = 240;
const MAX_RESULT_SNIPPET_CHARS_MEDIUM: usize = 480;
const MAX_RESULT_SNIPPET_CHARS_LONG: usize = 900;

const EXA_MCP_URL: &str = "https://mcp.exa.ai/mcp";
const BRAVE_SEARCH_URL: &str = "https://api.search.brave.com/res/v1/web/search";

fn default_num_results() -> u32 {
  DEFAULT_NUM_RESULTS
}

fn default_livecrawl() -> String {
  "fallback".to_string()
}

fn default_response_length() -> String {
  "medium".to_string()
}

#[derive(Debug, Deserialize)]
struct WebSearchArgs {
  query: String,
  #[serde(default = "default_num_results")]
  num_results: u32,
  #[serde(default = "default_livecrawl")]
  livecrawl: String,
  #[serde(default)]
  context_max_characters: Option<u32>,
  #[serde(default)]
  domains: Vec<String>,
  #[serde(default)]
  recency: Option<u32>,
  #[serde(default, rename = "type")]
  query_type: Option<String>,
  #[serde(default)]
  country: Option<String>,
  #[serde(default = "default_response_length")]
  response_length: String,
  #[serde(default)]
  images: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum WebSearchBackend {
  Brave,
  Searxng,
  Exa,
}

impl WebSearchBackend {
  fn as_str(self) -> &'static str {
    match self {
      Self::Brave => "brave",
      Self::Searxng => "searxng",
      Self::Exa => "exa",
    }
  }
}

#[derive(Debug, Serialize)]
struct WebSearchResponse {
  query: String,
  backend: String,
  backend_mode: String,
  result_count: usize,
  fetched_at: String,
  response_length: String,
  requested_result_count: u32,
  citation_ready: bool,
  used_provider_native: bool,
  results: Vec<WebSearchResult>,
  images: Vec<WebSearchImage>,
}

#[derive(Debug, Serialize)]
struct WebSearchResult {
  rank: usize,
  title: Option<String>,
  url: Option<String>,
  domain: Option<String>,
  snippet: String,
  http_status: Option<u16>,
  truncated: bool,
  fetched_at: String,
  citation: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct WebSearchImage {
  title: String,
  url: String,
}

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
  arguments: serde_json::Value,
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
    let query = args.query.trim();
    if query.is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "query must not be empty".to_string(),
      ));
    }

    let client = reqwest::Client::builder()
      .timeout(std::time::Duration::from_secs(SEARCH_TIMEOUT_SECS))
      .user_agent("Mozilla/5.0 (compatible; cokra/1.0)")
      .build()
      .map_err(|err| FunctionCallError::Execution(format!("failed to build HTTP client: {err}")))?;

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

    let content = serde_json::to_string(&output).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize web_search result: {err}"))
    })?;
    Ok(ToolOutput::success(content).with_id(id))
  }
}

async fn search_exa(
  client: &reqwest::Client,
  args: &WebSearchArgs,
) -> Result<WebSearchResponse, FunctionCallError> {
  let request = ExaMcpRequest {
    jsonrpc: "2.0",
    id: 1,
    method: "tools/call",
    params: ExaMcpParams {
      name: "web_search_exa",
      arguments: serde_json::json!({
        "query": decorate_query(args),
        "numResults": args.num_results.min(20),
        "livecrawl": args.livecrawl,
        "contextMaxCharacters": args.context_max_characters.unwrap_or(DEFAULT_CONTEXT_MAX_CHARACTERS),
        "country": args.country,
      }),
    },
  };

  let response = client
    .post(EXA_MCP_URL)
    .header("accept", "application/json, text/event-stream")
    .header("content-type", "application/json")
    .json(&request)
    .send()
    .await
    .map_err(|err| {
      FunctionCallError::RespondToModel(format!("Exa search request failed: {err}"))
    })?;

  if !response.status().is_success() {
    return Err(FunctionCallError::RespondToModel(format!(
      "Exa search failed with status {}",
      response.status()
    )));
  }

  let body = response
    .text()
    .await
    .map_err(|err| FunctionCallError::Execution(format!("failed to read Exa response: {err}")))?;
  let snippet = parse_exa_response(&body)?;
  let fetched_at = chrono::Utc::now().to_rfc3339();
  let (snippet, truncated) = truncate_snippet(&snippet, snippet_limit(&args.response_length));

  Ok(WebSearchResponse {
    query: args.query.clone(),
    backend: WebSearchBackend::Exa.as_str().to_string(),
    backend_mode: "local_fallback".to_string(),
    result_count: 1,
    fetched_at: fetched_at.clone(),
    response_length: args.response_length.clone(),
    requested_result_count: args.num_results,
    citation_ready: true,
    used_provider_native: false,
    results: vec![WebSearchResult {
      rank: 1,
      title: Some(args.query.clone()),
      url: None,
      domain: None,
      snippet,
      http_status: Some(200),
      truncated,
      fetched_at,
      citation: serde_json::json!({
        "backend": "exa",
        "query": args.query,
      }),
    }],
    images: Vec::new(),
  })
}

async fn search_brave(
  client: &reqwest::Client,
  args: &WebSearchArgs,
  api_key: &str,
) -> Result<WebSearchResponse, FunctionCallError> {
  let mut query = vec![
    ("q".to_string(), decorate_query(args)),
    ("count".to_string(), args.num_results.min(20).to_string()),
  ];
  if let Some(country) = args
    .country
    .as_deref()
    .filter(|country| !country.is_empty())
  {
    query.push(("country".to_string(), country.to_string()));
  }

  let response = client
    .get(BRAVE_SEARCH_URL)
    .header("Accept", "application/json")
    .header("Accept-Encoding", "gzip")
    .header("X-Subscription-Token", api_key)
    .query(&query)
    .send()
    .await
    .map_err(|err| {
      FunctionCallError::RespondToModel(format!("Brave search request failed: {err}"))
    })?;

  if !response.status().is_success() {
    return Err(FunctionCallError::RespondToModel(format!(
      "Brave search failed with status {}",
      response.status()
    )));
  }

  let resp: BraveSearchResponse = response.json().await.map_err(|err| {
    FunctionCallError::Execution(format!("failed to parse Brave response: {err}"))
  })?;
  let fetched_at = chrono::Utc::now().to_rfc3339();
  let results = resp
    .web
    .map(|web| web.results)
    .unwrap_or_default()
    .into_iter()
    .filter(|result| matches_domain_filter(&result.url, &args.domains))
    .take(args.num_results.min(20) as usize)
    .enumerate()
    .map(|(index, result)| {
      let (snippet, truncated) = truncate_snippet(
        result.description.as_deref().unwrap_or_default(),
        snippet_limit(&args.response_length),
      );
      WebSearchResult {
        rank: index + 1,
        title: Some(result.title.clone()),
        url: Some(result.url.clone()),
        domain: url_domain(&result.url),
        snippet,
        http_status: Some(200),
        truncated,
        fetched_at: fetched_at.clone(),
        citation: serde_json::json!({
          "url": result.url,
          "title": result.title,
          "backend": "brave",
        }),
      }
    })
    .collect::<Vec<_>>();

  Ok(WebSearchResponse {
    query: args.query.clone(),
    backend: WebSearchBackend::Brave.as_str().to_string(),
    backend_mode: "local_fallback".to_string(),
    result_count: results.len(),
    fetched_at,
    response_length: args.response_length.clone(),
    requested_result_count: args.num_results,
    citation_ready: true,
    used_provider_native: false,
    results,
    images: Vec::new(),
  })
}

async fn search_searxng(
  client: &reqwest::Client,
  args: &WebSearchArgs,
  base_url: &str,
) -> Result<WebSearchResponse, FunctionCallError> {
  let search_url = format!("{}/search", base_url.trim_end_matches('/'));
  let response = client
    .get(&search_url)
    .query(&[
      ("q", decorate_query(args)),
      ("format", "json".to_string()),
      ("engines", "google,bing,duckduckgo".to_string()),
    ])
    .send()
    .await
    .map_err(|err| {
      FunctionCallError::RespondToModel(format!("SearXNG search request failed: {err}"))
    })?;

  if !response.status().is_success() {
    return Err(FunctionCallError::RespondToModel(format!(
      "SearXNG search failed with status {}",
      response.status()
    )));
  }

  let resp: SearxngResponse = response.json().await.map_err(|err| {
    FunctionCallError::Execution(format!("failed to parse SearXNG response: {err}"))
  })?;
  let fetched_at = chrono::Utc::now().to_rfc3339();
  let results = resp
    .results
    .into_iter()
    .filter(|result| matches_domain_filter(&result.url, &args.domains))
    .take(args.num_results.min(20) as usize)
    .enumerate()
    .map(|(index, result)| {
      let (snippet, truncated) = truncate_snippet(
        result.content.as_deref().unwrap_or_default(),
        snippet_limit(&args.response_length),
      );
      WebSearchResult {
        rank: index + 1,
        title: Some(result.title.clone()),
        url: Some(result.url.clone()),
        domain: url_domain(&result.url),
        snippet,
        http_status: Some(200),
        truncated,
        fetched_at: fetched_at.clone(),
        citation: serde_json::json!({
          "url": result.url,
          "title": result.title,
          "backend": "searxng",
        }),
      }
    })
    .collect::<Vec<_>>();

  Ok(WebSearchResponse {
    query: args.query.clone(),
    backend: WebSearchBackend::Searxng.as_str().to_string(),
    backend_mode: "local_fallback".to_string(),
    result_count: results.len(),
    fetched_at,
    response_length: args.response_length.clone(),
    requested_result_count: args.num_results,
    citation_ready: true,
    used_provider_native: false,
    results,
    images: Vec::new(),
  })
}

fn parse_exa_response(body: &str) -> Result<String, FunctionCallError> {
  for line in body.lines() {
    if let Some(json_str) = line.strip_prefix("data: ")
      && let Ok(resp) = serde_json::from_str::<ExaMcpResponse>(json_str)
    {
      return extract_exa_text(resp);
    }
  }

  if let Ok(resp) = serde_json::from_str::<ExaMcpResponse>(body) {
    return extract_exa_text(resp);
  }

  Err(FunctionCallError::RespondToModel(
    "unexpected Exa response format".to_string(),
  ))
}

fn extract_exa_text(resp: ExaMcpResponse) -> Result<String, FunctionCallError> {
  if let Some(err) = resp.error {
    return Err(FunctionCallError::RespondToModel(format!(
      "Exa search error: {}",
      err.message
    )));
  }

  let result = resp.result.ok_or_else(|| {
    FunctionCallError::RespondToModel("Exa search returned no result".to_string())
  })?;
  Ok(
    result
      .content
      .into_iter()
      .find(|item| item.kind == "text")
      .and_then(|item| item.text)
      .unwrap_or_else(|| "No search results found".to_string()),
  )
}

fn decorate_query(args: &WebSearchArgs) -> String {
  let mut query = args.query.trim().to_string();
  if let Some(query_type) = args.query_type.as_deref().filter(|value| !value.is_empty()) {
    query.push(' ');
    query.push_str(query_type);
  }
  if let Some(recency) = args.recency {
    query.push_str(&format!(" last {recency} days"));
  }
  if args.domains.len() == 1 {
    query.push_str(&format!(" site:{}", args.domains[0]));
  } else if !args.domains.is_empty() {
    let domains = args
      .domains
      .iter()
      .map(|domain| format!("site:{domain}"))
      .collect::<Vec<_>>();
    query.push_str(&format!(" ({})", domains.join(" OR ")));
  }
  query
}

fn snippet_limit(response_length: &str) -> usize {
  match response_length {
    "short" => MAX_RESULT_SNIPPET_CHARS_SHORT,
    "long" => MAX_RESULT_SNIPPET_CHARS_LONG,
    _ => MAX_RESULT_SNIPPET_CHARS_MEDIUM,
  }
}

fn truncate_snippet(snippet: &str, max_chars: usize) -> (String, bool) {
  if snippet.len() <= max_chars {
    return (snippet.to_string(), false);
  }
  let boundary = snippet.floor_char_boundary(max_chars);
  (snippet[..boundary].to_string(), true)
}

fn matches_domain_filter(url: &str, domains: &[String]) -> bool {
  if domains.is_empty() {
    return true;
  }
  let Some(domain) = url_domain(url) else {
    return false;
  };
  domains.iter().any(|candidate| {
    let candidate = candidate
      .trim()
      .trim_start_matches("*.")
      .to_ascii_lowercase();
    domain == candidate || domain.ends_with(&format!(".{candidate}"))
  })
}

fn url_domain(url: &str) -> Option<String> {
  reqwest::Url::parse(url)
    .ok()
    .and_then(|url| url.host_str().map(|host| host.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
  use super::*;
  use tokio::io::AsyncReadExt;
  use tokio::io::AsyncWriteExt;
  use tokio::net::TcpListener;

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

  async fn spawn_http_server(status: &str, headers: &[(&str, String)], body: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let status = status.to_string();
    let headers = headers
      .iter()
      .map(|(name, value)| ((*name).to_string(), value.clone()))
      .collect::<Vec<_>>();
    tokio::spawn(async move {
      let (mut stream, _) = listener.accept().await.expect("accept");
      let mut buffer = [0u8; 2048];
      let _ = stream.read(&mut buffer).await;
      let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.as_bytes().len()
      );
      for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
      }
      response.push_str("\r\n");
      response.push_str(&body);
      stream
        .write_all(response.as_bytes())
        .await
        .expect("write response");
    });
    format!("http://{addr}")
  }

  #[tokio::test]
  async fn rejects_empty_query() {
    let err = WebSearchHandler
      .handle_async(make_inv(serde_json::json!({ "query": " " })))
      .await
      .unwrap_err();
    assert!(err.to_string().contains("empty"));
  }

  #[test]
  fn decorate_query_includes_domain_and_recency() {
    let args: WebSearchArgs = serde_json::from_value(serde_json::json!({
      "query": "rust lsp",
      "domains": ["docs.rs"],
      "recency": 7
    }))
    .expect("args");
    let query = decorate_query(&args);
    assert!(query.contains("site:docs.rs"));
    assert!(query.contains("last 7 days"));
  }

  #[test]
  fn parse_exa_plain_json_response() {
    let body =
      r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"Hello world"}]}}"#;
    let result = parse_exa_response(body).expect("exa response");
    assert!(result.contains("Hello world"));
  }

  #[test]
  fn parse_exa_error_response() {
    let body = r#"{"jsonrpc":"2.0","id":1,"error":{"message":"Rate limit exceeded"}}"#;
    let err = parse_exa_response(body).unwrap_err();
    assert!(err.to_string().contains("Rate limit exceeded"));
  }

  #[test]
  fn domain_filter_matches_subdomains() {
    assert!(matches_domain_filter(
      "https://docs.rs/reqwest",
      &["docs.rs".to_string()]
    ));
    assert!(!matches_domain_filter(
      "https://example.com",
      &["docs.rs".to_string()]
    ));
  }

  #[tokio::test]
  async fn search_searxng_returns_structured_results() {
    let base_url = spawn_http_server(
      "200 OK",
      &[("Content-Type", "application/json".to_string())],
      serde_json::json!({
        "results": [
          {
            "title": "Reqwest docs",
            "url": "https://docs.rs/reqwest/latest/reqwest/",
            "content": "Rust HTTP client documentation"
          },
          {
            "title": "Ignored result",
            "url": "https://example.com/ignored",
            "content": "Should be filtered out"
          }
        ]
      })
      .to_string(),
    )
    .await;
    let client = reqwest::Client::builder()
      .timeout(std::time::Duration::from_secs(5))
      .build()
      .expect("client");
    let args: WebSearchArgs = serde_json::from_value(serde_json::json!({
      "query": "reqwest",
      "domains": ["docs.rs"],
      "response_length": "short"
    }))
    .expect("args");

    let response = search_searxng(&client, &args, &base_url)
      .await
      .expect("search succeeds");

    assert_eq!(response.backend, "searxng");
    assert_eq!(response.backend_mode, "local_fallback");
    assert_eq!(response.result_count, 1);
    assert_eq!(response.results[0].domain.as_deref(), Some("docs.rs"));
    assert!(response.results[0].snippet.contains("Rust HTTP client"));
  }
}
