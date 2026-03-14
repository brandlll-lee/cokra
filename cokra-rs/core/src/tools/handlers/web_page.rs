use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

use super::web_fetch::WebFetchFormat;
use super::web_fetch::fetch_url_document;

pub struct WebOpenPageHandler;
pub struct WebFindInPageHandler;

fn default_open_format() -> String {
  "markdown".to_string()
}

fn default_max_matches() -> usize {
  20
}

#[derive(Debug, Deserialize)]
struct WebOpenPageArgs {
  url: String,
  #[serde(default = "default_open_format")]
  format: String,
  #[serde(default)]
  timeout: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct WebFindInPageArgs {
  url: String,
  pattern: String,
  #[serde(default)]
  timeout: Option<u64>,
  #[serde(default = "default_max_matches")]
  max_matches: usize,
}

#[derive(Debug, Serialize)]
struct WebFindInPageResponse {
  url: String,
  title: Option<String>,
  pattern: String,
  total_matches: usize,
  matches: Vec<WebPageMatch>,
}

#[derive(Debug, Serialize)]
struct WebPageMatch {
  line: usize,
  text: String,
}

#[async_trait]
impl ToolHandler for WebOpenPageHandler {
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
    let args: WebOpenPageArgs = invocation.parse_arguments()?;
    let format = WebFetchFormat::parse(&args.format).ok_or_else(|| {
      FunctionCallError::RespondToModel("format must be one of: text, markdown, html".to_string())
    })?;
    let document = fetch_url_document(&invocation, &args.url, format, args.timeout).await?;
    let content = serde_json::to_string(&document).map_err(|err| {
      FunctionCallError::Fatal(format!("failed to serialize web_open_page result: {err}"))
    })?;
    Ok(ToolOutput::success(content).with_id(id))
  }
}

#[async_trait]
impl ToolHandler for WebFindInPageHandler {
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
    let args: WebFindInPageArgs = invocation.parse_arguments()?;
    let pattern = args.pattern.trim();
    if pattern.is_empty() {
      return Err(FunctionCallError::RespondToModel(
        "pattern must not be empty".to_string(),
      ));
    }

    let document =
      fetch_url_document(&invocation, &args.url, WebFetchFormat::Text, args.timeout).await?;
    let needle = pattern.to_ascii_lowercase();
    let mut matches = document
      .content
      .lines()
      .enumerate()
      .filter_map(|(index, line)| {
        line
          .to_ascii_lowercase()
          .contains(&needle)
          .then(|| WebPageMatch {
            line: index + 1,
            text: line.to_string(),
          })
      })
      .collect::<Vec<_>>();

    let total_matches = matches.len();
    matches.truncate(args.max_matches.max(1));
    let content = serde_json::to_string(&WebFindInPageResponse {
      url: document.final_url,
      title: document.title,
      pattern: pattern.to_string(),
      total_matches,
      matches,
    })
    .map_err(|err| {
      FunctionCallError::Fatal(format!(
        "failed to serialize web_find_in_page result: {err}"
      ))
    })?;
    Ok(ToolOutput::success(content).with_id(id))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::tools::context::ToolPayload;

  fn make_inv(name: &str, args: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
      id: "1".to_string(),
      name: name.to_string(),
      payload: ToolPayload::Function {
        arguments: args.to_string(),
      },
      cwd: std::env::temp_dir(),
      runtime: None,
    }
  }

  #[tokio::test]
  async fn web_find_in_page_rejects_empty_pattern() {
    let err = WebFindInPageHandler
      .handle_async(make_inv(
        "web_find_in_page",
        serde_json::json!({
          "url": "https://example.com",
          "pattern": " "
        }),
      ))
      .await
      .unwrap_err();
    assert!(err.to_string().contains("pattern"));
  }

  #[tokio::test]
  async fn web_open_page_rejects_invalid_format() {
    let err = WebOpenPageHandler
      .handle_async(make_inv(
        "web_open_page",
        serde_json::json!({
          "url": "https://example.com",
          "format": "pdf"
        }),
      ))
      .await
      .unwrap_err();
    assert!(err.to_string().contains("format"));
  }
}
