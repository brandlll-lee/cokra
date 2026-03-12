//! web_fetch tool handler — HTTP GET with HTML→text conversion.
//!
//! Modelled after OpenCode's `webfetch` and Gemini CLI's `web_fetch` tools.
//! Fetches a URL and returns content as plain text or raw HTML.

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct WebFetchHandler;

const MAX_RESPONSE_BYTES: usize = 5 * 1024 * 1024; // 5 MB
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;

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

    // Validate URL
    if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
      return Err(FunctionCallError::RespondToModel(
        "url must start with http:// or https://".to_string(),
      ));
    }

    // Validate format
    let format = args.format.to_lowercase();
    if !["text", "html", "raw"].contains(&format.as_str()) {
      return Err(FunctionCallError::RespondToModel(
        "format must be one of: text, html, raw".to_string(),
      ));
    }

    let timeout_secs = args
      .timeout
      .unwrap_or(DEFAULT_TIMEOUT_SECS)
      .min(MAX_TIMEOUT_SECS);

    let client = reqwest::Client::builder()
      .timeout(std::time::Duration::from_secs(timeout_secs))
      .user_agent("Mozilla/5.0 (compatible; cokra/1.0; +https://github.com/nicepkg/cokra)")
      .redirect(reqwest::redirect::Policy::limited(10))
      .build()
      .map_err(|e| FunctionCallError::Execution(format!("failed to build HTTP client: {e}")))?;

    let response = client
      .get(&args.url)
      .header("Accept", "text/html,application/xhtml+xml,text/plain,*/*")
      .header("Accept-Language", "en-US,en;q=0.9")
      .send()
      .await
      .map_err(|e| FunctionCallError::RespondToModel(format!("HTTP request failed: {e}")))?;

    let status = response.status();
    if !status.is_success() {
      return Err(FunctionCallError::RespondToModel(format!(
        "HTTP request failed with status {status}"
      )));
    }

    let content_type = response
      .headers()
      .get("content-type")
      .and_then(|v| v.to_str().ok())
      .unwrap_or("")
      .to_string();

    // Check content length header
    if let Some(len) = response.content_length()
      && len as usize > MAX_RESPONSE_BYTES
    {
      return Err(FunctionCallError::RespondToModel(format!(
        "Response too large ({} bytes, max {})",
        len, MAX_RESPONSE_BYTES
      )));
    }

    let bytes = response
      .bytes()
      .await
      .map_err(|e| FunctionCallError::Execution(format!("failed to read response body: {e}")))?;

    if bytes.len() > MAX_RESPONSE_BYTES {
      return Err(FunctionCallError::RespondToModel(format!(
        "Response too large ({} bytes, max {})",
        bytes.len(),
        MAX_RESPONSE_BYTES
      )));
    }

    let body = String::from_utf8_lossy(&bytes).to_string();

    let output = match format.as_str() {
      "html" | "raw" => body,
      "text" | _ => {
        if content_type.contains("text/html") {
          html_to_text(&body)
        } else {
          body
        }
      }
    };

    // Truncate very large text output for the model
    let max_chars = 200_000;
    let (final_output, was_truncated) = if output.len() > max_chars {
      let truncated = &output[..output.floor_char_boundary(max_chars)];
      (
        format!("{truncated}\n\n[Content truncated at {max_chars} characters]"),
        true,
      )
    } else {
      (output, false)
    };

    let title = format!(
      "{} ({}){}",
      args.url,
      content_type,
      if was_truncated { " [truncated]" } else { "" }
    );

    Ok(ToolOutput::success(format!("Fetched: {title}\n\n{final_output}")).with_id(id))
  }
}

/// Simple HTML to text conversion.
/// Strips tags, decodes common HTML entities, and normalises whitespace.
fn html_to_text(html: &str) -> String {
  // Remove script and style blocks
  let mut s = html.to_string();

  // Remove <script>...</script> and <style>...</style> blocks (case-insensitive)
  for tag in &["script", "style", "noscript"] {
    loop {
      let lower = s.to_lowercase();
      if let Some(start) = lower.find(&format!("<{tag}")) {
        if let Some(end) = lower[start..].find(&format!("</{tag}>")) {
          let end_abs = start + end + format!("</{tag}>").len();
          s.replace_range(start..end_abs, " ");
        } else {
          break;
        }
      } else {
        break;
      }
    }
  }

  // Replace block-level tags with newlines
  for tag in &[
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
    s = s.replace(&format!("<{tag}"), &format!("\n<{tag}"));
    s = s.replace(&format!("</{tag}>"), &format!("</{tag}>\n"));
  }

  // Strip remaining tags
  let mut result = String::with_capacity(s.len());
  let mut in_tag = false;
  for ch in s.chars() {
    match ch {
      '<' => in_tag = true,
      '>' => in_tag = false,
      _ if !in_tag => result.push(ch),
      _ => {}
    }
  }

  // Decode common HTML entities
  let result = result
    .replace("&amp;", "&")
    .replace("&lt;", "<")
    .replace("&gt;", ">")
    .replace("&quot;", "\"")
    .replace("&#39;", "'")
    .replace("&apos;", "'")
    .replace("&nbsp;", " ")
    .replace("&#x27;", "'")
    .replace("&#x2F;", "/")
    .replace("&mdash;", "—")
    .replace("&ndash;", "–")
    .replace("&hellip;", "…");

  // Normalise whitespace: collapse multiple blank lines, trim each line
  let lines: Vec<&str> = result.lines().map(|l| l.trim()).collect();
  let mut output = Vec::new();
  let mut prev_empty = false;
  for line in lines {
    if line.is_empty() {
      if !prev_empty {
        output.push("");
      }
      prev_empty = true;
    } else {
      output.push(line);
      prev_empty = false;
    }
  }

  output.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn html_to_text_strips_tags() {
    let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
    let text = html_to_text(html);
    assert!(text.contains("Hello"));
    assert!(text.contains("World"));
    assert!(!text.contains("<"));
  }

  #[test]
  fn html_to_text_removes_scripts() {
    let html = "<p>before</p><script>alert('xss')</script><p>after</p>";
    let text = html_to_text(html);
    assert!(text.contains("before"));
    assert!(text.contains("after"));
    assert!(!text.contains("alert"));
  }

  #[test]
  fn html_to_text_decodes_entities() {
    let html = "&amp; &lt; &gt; &quot; &#39;";
    let text = html_to_text(html);
    assert!(text.contains("& < > \" '"));
  }

  #[test]
  fn html_to_text_handles_empty() {
    assert_eq!(html_to_text(""), "");
  }

  #[test]
  fn html_to_text_preserves_plain_text() {
    let text = "Just plain text without HTML";
    assert_eq!(html_to_text(text), text);
  }

  // Integration tests for the handler require network access;
  // we test argument validation here.
  fn make_inv(args: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
      id: "1".to_string(),
      name: "web_fetch".to_string(),
      payload: crate::tools::context::ToolPayload::Function {
        arguments: args.to_string(),
      },
      cwd: std::env::temp_dir(),
      runtime: None,
    }
  }

  #[tokio::test]
  async fn rejects_non_http_url() {
    let inv = make_inv(serde_json::json!({
      "url": "ftp://example.com"
    }));
    let err = WebFetchHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("http://"));
  }

  #[tokio::test]
  async fn rejects_invalid_format() {
    let inv = make_inv(serde_json::json!({
      "url": "https://example.com",
      "format": "pdf"
    }));
    let err = WebFetchHandler.handle_async(inv).await.unwrap_err();
    assert!(err.to_string().contains("format"));
  }

  #[tokio::test]
  async fn default_format_is_text() {
    // Just verify args parse correctly with default format
    let args: WebFetchArgs = serde_json::from_value(serde_json::json!({
      "url": "https://example.com"
    }))
    .unwrap();
    assert_eq!(args.format, "text");
  }
}
