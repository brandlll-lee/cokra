use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct WebFetchHandler;

const MAX_RESPONSE_BYTES: usize = 5 * 1024 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;
const DEFAULT_MAX_CHARS: usize = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebFetchFormat {
  Text,
  Markdown,
  Html,
}

impl WebFetchFormat {
  fn as_str(self) -> &'static str {
    match self {
      Self::Text => "text",
      Self::Markdown => "markdown",
      Self::Html => "html",
    }
  }

  pub(crate) fn parse(raw: &str) -> Option<Self> {
    match raw.trim().to_ascii_lowercase().as_str() {
      "text" => Some(Self::Text),
      "markdown" => Some(Self::Markdown),
      "html" | "raw" => Some(Self::Html),
      _ => None,
    }
  }
}

fn default_format() -> String {
  "text".to_string()
}

#[derive(Debug, Deserialize)]
struct WebFetchArgs {
  url: String,
  #[serde(default = "default_format")]
  format: String,
  #[serde(default)]
  timeout: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WebPageDocument {
  pub backend: String,
  pub url: String,
  pub final_url: String,
  pub format: String,
  pub title: Option<String>,
  pub http_status: u16,
  pub content_type: String,
  pub charset: Option<String>,
  pub fetched_at: String,
  pub truncated: bool,
  pub content_length: usize,
  pub content: String,
}

#[async_trait]
impl ToolHandler for WebFetchHandler {
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
    let args: WebFetchArgs = invocation.parse_arguments()?;
    let format = WebFetchFormat::parse(&args.format).ok_or_else(|| {
      FunctionCallError::RespondToModel("format must be one of: text, markdown, html".to_string())
    })?;

    let document = fetch_url_document(&invocation, &args.url, format, args.timeout).await?;
    let content = serde_json::to_string(&document).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize web_fetch result: {err}"))
    })?;
    Ok(ToolOutput::success(content).with_id(id))
  }
}

pub(crate) async fn fetch_url_document(
  invocation: &ToolInvocation,
  url: &str,
  format: WebFetchFormat,
  timeout: Option<u64>,
) -> Result<WebPageDocument, FunctionCallError> {
  if !url.starts_with("http://") && !url.starts_with("https://") {
    return Err(FunctionCallError::RespondToModel(
      "url must start with http:// or https://".to_string(),
    ));
  }

  if let Some(runtime) = invocation.runtime.as_ref() {
    crate::tools::network_approval::authorize_http_url(runtime.as_ref(), &invocation.cwd, url, &[])
      .await
      .map_err(FunctionCallError::RespondToModel)?;
  }

  let timeout_secs = timeout
    .unwrap_or(DEFAULT_TIMEOUT_SECS)
    .clamp(1, MAX_TIMEOUT_SECS);
  let client = reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(timeout_secs))
    .user_agent("Mozilla/5.0 (compatible; cokra/1.0; +https://github.com/cokra/cokra)")
    .redirect(reqwest::redirect::Policy::limited(10))
    .build()
    .map_err(|err| FunctionCallError::Execution(format!("failed to build HTTP client: {err}")))?;

  let response = client
    .get(url)
    .header("Accept", "text/html,application/xhtml+xml,text/plain,*/*")
    .header("Accept-Language", "en-US,en;q=0.9")
    .send()
    .await
    .map_err(|err| FunctionCallError::RespondToModel(format!("HTTP request failed: {err}")))?;

  let status = response.status();
  let final_url = response.url().to_string();
  let content_type = response
    .headers()
    .get("content-type")
    .and_then(|value| value.to_str().ok())
    .unwrap_or("")
    .to_string();
  let charset = parse_charset(&content_type);

  if !status.is_success() {
    let cloudflare = looks_like_cloudflare_block(status.as_u16(), &content_type, &final_url);
    let detail = if cloudflare {
      "request looks like a Cloudflare or anti-bot block"
    } else {
      "request was not successful"
    };
    return Err(FunctionCallError::RespondToModel(format!(
      "HTTP request failed with status {status} ({detail})"
    )));
  }

  if let Some(length) = response.content_length()
    && length as usize > MAX_RESPONSE_BYTES
  {
    return Err(FunctionCallError::RespondToModel(format!(
      "Response too large ({} bytes, max {})",
      length, MAX_RESPONSE_BYTES
    )));
  }

  let bytes = response
    .bytes()
    .await
    .map_err(|err| FunctionCallError::Execution(format!("failed to read response body: {err}")))?;
  if bytes.len() > MAX_RESPONSE_BYTES {
    return Err(FunctionCallError::RespondToModel(format!(
      "Response too large ({} bytes, max {})",
      bytes.len(),
      MAX_RESPONSE_BYTES
    )));
  }

  let body = String::from_utf8_lossy(&bytes).to_string();
  let title = extract_title(&body);
  let rendered = match format {
    WebFetchFormat::Html => body.clone(),
    WebFetchFormat::Markdown => {
      if content_type.contains("html") {
        html_to_markdown(&body, title.as_deref())
      } else {
        body.clone()
      }
    }
    WebFetchFormat::Text => {
      if content_type.contains("html") {
        html_to_text(&body)
      } else {
        body.clone()
      }
    }
  };
  let (content, truncated) = truncate_text(rendered, DEFAULT_MAX_CHARS);

  Ok(WebPageDocument {
    backend: "http_fetch".to_string(),
    url: url.to_string(),
    final_url,
    format: format.as_str().to_string(),
    title,
    http_status: status.as_u16(),
    content_type,
    charset,
    fetched_at: chrono::Utc::now().to_rfc3339(),
    truncated,
    content_length: bytes.len(),
    content,
  })
}

fn truncate_text(text: String, max_chars: usize) -> (String, bool) {
  if text.len() <= max_chars {
    return (text, false);
  }
  let boundary = text.floor_char_boundary(max_chars);
  (
    format!(
      "{}\n\n[Content truncated at {max_chars} characters]",
      &text[..boundary]
    ),
    true,
  )
}

fn parse_charset(content_type: &str) -> Option<String> {
  content_type.split(';').skip(1).find_map(|part| {
    let (key, value) = part.trim().split_once('=')?;
    (key.trim().eq_ignore_ascii_case("charset")).then(|| value.trim().to_string())
  })
}

fn looks_like_cloudflare_block(status: u16, content_type: &str, final_url: &str) -> bool {
  status == 403
    && (content_type.contains("html")
      || final_url.contains("/cdn-cgi/")
      || final_url.contains("cloudflare"))
}

fn extract_title(html: &str) -> Option<String> {
  let lower = html.to_ascii_lowercase();
  let start = lower.find("<title>")?;
  let end = lower[start + 7..].find("</title>")?;
  let title = html[start + 7..start + 7 + end].trim();
  (!title.is_empty()).then(|| html_entity_decode(title))
}

fn html_entity_decode(text: &str) -> String {
  text
    .replace("&amp;", "&")
    .replace("&lt;", "<")
    .replace("&gt;", ">")
    .replace("&quot;", "\"")
    .replace("&#39;", "'")
    .replace("&apos;", "'")
    .replace("&nbsp;", " ")
    .replace("&#x27;", "'")
    .replace("&#x2F;", "/")
    .replace("&mdash;", "-")
    .replace("&ndash;", "-")
    .replace("&hellip;", "...")
}

pub(crate) fn html_to_markdown(html: &str, title: Option<&str>) -> String {
  let mut parts = Vec::new();
  if let Some(title) = title.filter(|title| !title.is_empty()) {
    parts.push(format!("# {title}"));
  }
  let text = html_to_text(html);
  if !text.is_empty() {
    parts.push(text);
  }
  parts.join("\n\n")
}

pub(crate) fn html_to_text(html: &str) -> String {
  let mut text = html.to_string();

  for tag in ["script", "style", "noscript"] {
    loop {
      let lower = text.to_ascii_lowercase();
      let Some(start) = lower.find(&format!("<{tag}")) else {
        break;
      };
      let Some(end) = lower[start..].find(&format!("</{tag}>")) else {
        break;
      };
      let end = start + end + tag.len() + 3;
      text.replace_range(start..end, " ");
    }
  }

  for tag in [
    "br",
    "p",
    "div",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "li",
    "tr",
    "hr",
    "blockquote",
    "section",
    "article",
    "header",
    "footer",
    "nav",
    "main",
  ] {
    text = text.replace(&format!("<{tag}"), &format!("\n<{tag}"));
    text = text.replace(&format!("</{tag}>"), &format!("</{tag}>\n"));
  }

  let mut stripped = String::with_capacity(text.len());
  let mut in_tag = false;
  for ch in text.chars() {
    match ch {
      '<' => in_tag = true,
      '>' => in_tag = false,
      _ if !in_tag => stripped.push(ch),
      _ => {}
    }
  }

  let decoded = html_entity_decode(&stripped);
  let mut lines = Vec::new();
  let mut previous_empty = false;
  for line in decoded.lines().map(str::trim) {
    if line.is_empty() {
      if !previous_empty {
        lines.push(String::new());
      }
      previous_empty = true;
    } else {
      lines.push(line.to_string());
      previous_empty = false;
    }
  }

  lines.join("\n").trim().to_string()
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
      id: "1".to_string(),
      name: "web_fetch".to_string(),
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

  #[test]
  fn html_to_text_strips_tags() {
    let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
    let text = html_to_text(html);
    assert!(text.contains("Hello"));
    assert!(text.contains("World"));
    assert!(!text.contains('<'));
  }

  #[test]
  fn html_to_markdown_includes_title() {
    let markdown = html_to_markdown("<p>World</p>", Some("Hello"));
    assert!(markdown.contains("# Hello"));
    assert!(markdown.contains("World"));
  }

  #[test]
  fn extract_title_reads_html_title() {
    assert_eq!(
      extract_title("<html><head><title>Example</title></head></html>").as_deref(),
      Some("Example")
    );
  }

  #[tokio::test]
  async fn rejects_non_http_url() {
    let err = WebFetchHandler
      .handle_async(make_inv(serde_json::json!({
        "url": "ftp://example.com"
      })))
      .await
      .unwrap_err();
    assert!(err.to_string().contains("http://"));
  }

  #[tokio::test]
  async fn rejects_invalid_format() {
    let err = WebFetchHandler
      .handle_async(make_inv(serde_json::json!({
        "url": "https://example.com",
        "format": "pdf"
      })))
      .await
      .unwrap_err();
    assert!(err.to_string().contains("format"));
  }

  #[test]
  fn parse_charset_finds_charset_parameter() {
    assert_eq!(
      parse_charset("text/html; charset=utf-8").as_deref(),
      Some("utf-8")
    );
  }

  #[tokio::test]
  async fn fetch_url_document_follows_redirects_and_returns_markdown() {
    let final_base = spawn_http_server(
      "200 OK",
      &[("Content-Type", "text/html; charset=utf-8".to_string())],
      "<html><head><title>Redirected Page</title></head><body><p>Hello from redirect</p></body></html>"
        .to_string(),
    )
    .await;
    let redirect_base = spawn_http_server(
      "302 Found",
      &[("Location", format!("{final_base}/page"))],
      String::new(),
    )
    .await;
    let invocation = make_inv(serde_json::json!({
      "url": format!("{redirect_base}/start"),
      "format": "markdown"
    }));

    let document = fetch_url_document(
      &invocation,
      &format!("{redirect_base}/start"),
      WebFetchFormat::Markdown,
      Some(5),
    )
    .await
    .expect("fetch succeeds");

    assert_eq!(document.title.as_deref(), Some("Redirected Page"));
    assert!(document.final_url.starts_with(&final_base));
    assert_eq!(document.format, "markdown");
    assert!(document.content.contains("# Redirected Page"));
    assert!(document.content.contains("Hello from redirect"));
  }

  #[tokio::test]
  async fn fetch_url_document_reports_cloudflare_like_blocks() {
    let base = spawn_http_server(
      "403 Forbidden",
      &[("Content-Type", "text/html".to_string())],
      "<html><body>blocked</body></html>".to_string(),
    )
    .await;
    let invocation = make_inv(serde_json::json!({
      "url": format!("{base}/cdn-cgi/challenge-platform"),
      "format": "text"
    }));
    let err = fetch_url_document(
      &invocation,
      &format!("{base}/cdn-cgi/challenge-platform"),
      WebFetchFormat::Text,
      Some(5),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("Cloudflare"));
  }
}
