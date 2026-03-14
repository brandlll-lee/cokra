use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Method;

use crate::integrations::loader::LoadedIntegrationManifest;
use crate::integrations::manifest::IntegrationKind;
use crate::integrations::manifest::IntegrationToolExecution;
use crate::integrations::projector::RegisteredIntegrationTool;
use crate::integrations::providers::cli::definition_from_manifest;
use crate::integrations::providers::cli::render_json_template;
use crate::integrations::providers::cli::render_string_template;
use crate::integrations::providers::cli::spec_from_manifest;
use crate::tool_runtime::ToolSource;
use crate::tools::ToolHandler;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolKind;
use crate::tools::spec::ToolSourceKind;

pub fn project_api_tools(
  manifests: &[&LoadedIntegrationManifest],
) -> anyhow::Result<Vec<RegisteredIntegrationTool>> {
  let mut projected = Vec::new();
  for loaded in manifests {
    if loaded.manifest.kind != IntegrationKind::Api {
      continue;
    }
    for tool in &loaded.manifest.tools {
      let IntegrationToolExecution::Http {
        method,
        url,
        headers,
        query,
        body,
        timeout_ms,
      } = &tool.execution
      else {
        continue;
      };
      projected.push(RegisteredIntegrationTool {
        spec: spec_from_manifest(tool, ToolSourceKind::Api),
        definition: definition_from_manifest(
          &loaded.manifest.name,
          tool,
          ToolSource::Api,
          ToolSourceKind::Api,
        ),
        handler: Arc::new(ManifestApiHandler {
          method: method.clone(),
          url: url.clone(),
          headers: headers.clone(),
          query: query.clone(),
          body: body.clone(),
          timeout_ms: *timeout_ms,
        }),
      });
    }
  }
  Ok(projected)
}

struct ManifestApiHandler {
  method: String,
  url: String,
  headers: HashMap<String, String>,
  query: HashMap<String, String>,
  body: Option<serde_json::Value>,
  timeout_ms: Option<u64>,
}

#[async_trait]
impl ToolHandler for ManifestApiHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> std::result::Result<ToolOutput, FunctionCallError> {
    let input = invocation.parse_arguments_value()?;
    let client = reqwest::Client::builder()
      .timeout(
        self
          .timeout_ms
          .map(std::time::Duration::from_millis)
          .unwrap_or_else(|| std::time::Duration::from_secs(15)),
      )
      .build()
      .map_err(|err| FunctionCallError::Execution(format!("failed to build HTTP client: {err}")))?;
    let method = Method::from_bytes(self.method.as_bytes()).map_err(|err| {
      FunctionCallError::Execution(format!("invalid HTTP method `{}`: {err}", self.method))
    })?;
    let url = render_string_template(&self.url, &input, invocation.cwd.as_path())?;
    let mut request = client.request(method, &url);

    for (key, value) in &self.headers {
      request = request.header(
        key,
        render_string_template(value, &input, invocation.cwd.as_path())?,
      );
    }
    if !self.query.is_empty() {
      let rendered = self
        .query
        .iter()
        .map(|(key, value)| {
          render_string_template(value, &input, invocation.cwd.as_path())
            .map(|rendered| (key.clone(), rendered))
        })
        .collect::<Result<Vec<_>, _>>()?;
      request = request.query(&rendered);
    }
    if let Some(body) = &self.body {
      request = request.json(&render_json_template(
        body,
        &input,
        invocation.cwd.as_path(),
      )?);
    }

    let response = request.send().await.map_err(|err| {
      FunctionCallError::Execution(format!("API integration request failed: {err}"))
    })?;
    let status = response.status();
    let text = response
      .text()
      .await
      .map_err(|err| FunctionCallError::Execution(format!("failed to read API response: {err}")))?;
    let body = serde_json::from_str::<serde_json::Value>(&text)
      .unwrap_or_else(|_| serde_json::Value::String(text.clone()));
    let content = serde_json::json!({
      "status": status.as_u16(),
      "ok": status.is_success(),
      "body": body,
    });
    Ok(
      if status.is_success() {
        ToolOutput::success(content.to_string())
      } else {
        ToolOutput::error(content.to_string())
      }
      .with_id(invocation.id),
    )
  }
}
