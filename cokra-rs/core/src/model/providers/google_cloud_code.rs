use std::collections::HashMap;
use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde::Serialize;

use crate::model::error::ModelError;
use crate::model::error::Result;
use crate::model::provider::ModelProvider;
use crate::model::types::ChatRequest;
use crate::model::types::ChatResponse;
use crate::model::types::Choice;
use crate::model::types::ChoiceMessage;
use crate::model::types::Chunk;
use crate::model::types::ContentDelta;
use crate::model::types::ListModelsResponse;
use crate::model::types::Message;
use crate::model::types::ModelInfo;
use crate::model::types::ProviderConfig;
use crate::model::types::Tool;
use crate::model::types::ToolCall;
use crate::model::types::ToolCallDelta;
use crate::model::types::ToolCallFunction;
use crate::model::types::ToolCallProviderMeta;
use crate::model::types::Usage;

use super::create_client;

const DEFAULT_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com";
const ANTIGRAVITY_DAILY_ENDPOINT: &str = "https://daily-cloudcode-pa.sandbox.googleapis.com";
const DEFAULT_ANTIGRAVITY_VERSION: &str = "1.15.8";
const CLAUDE_THINKING_BETA_HEADER: &str = "interleaved-thinking-2025-05-14";
const MAX_RETRIES: u32 = 3;
const BASE_DELAY_MS: u64 = 1000;
const MAX_EMPTY_STREAM_RETRIES: u32 = 2;
const EMPTY_STREAM_BASE_DELAY_MS: u64 = 500;
const ANTIGRAVITY_SYSTEM_INSTRUCTION: &str = "You are Antigravity, a powerful agentic AI coding assistant designed by the Google Deepmind team working on Advanced Agentic Coding.You are pair programming with a USER to solve their coding task. The task may require creating a new codebase, modifying or debugging an existing codebase, or simply answering a question.**Absolute paths only****Proactiveness**";

pub const GOOGLE_GEMINI_CLI_MODELS: &[&str] = &[
  "gemini-2.0-flash",
  "gemini-2.5-flash",
  "gemini-2.5-pro",
  "gemini-3-flash-preview",
  "gemini-3-pro-preview",
];

pub const GOOGLE_ANTIGRAVITY_MODELS: &[&str] = &[
  "gemini-3-flash",
  "gemini-3-pro-low",
  "gemini-3-pro-high",
  "claude-sonnet-4-5",
  "claude-sonnet-4-5-thinking",
  "claude-opus-4-5-thinking",
  "gpt-oss-120b-medium",
];

#[derive(Debug, Clone)]
pub struct GoogleCloudCodeProvider {
  client: Client,
  config: ProviderConfig,
  provider_id: &'static str,
  provider_name: &'static str,
  access_token: String,
  project_id: String,
  is_antigravity: bool,
  base_url: Option<String>,
}

impl GoogleCloudCodeProvider {
  pub fn new_gemini_cli(raw_credentials: String, config: ProviderConfig) -> Result<Self> {
    Self::new(
      "google-gemini-cli",
      "Google Cloud Code Assist (Gemini CLI)",
      raw_credentials,
      config,
      false,
    )
  }

  pub fn new_antigravity(raw_credentials: String, config: ProviderConfig) -> Result<Self> {
    Self::new(
      "google-antigravity",
      "Antigravity",
      raw_credentials,
      config,
      true,
    )
  }

  fn new(
    provider_id: &'static str,
    provider_name: &'static str,
    raw_credentials: String,
    config: ProviderConfig,
    is_antigravity: bool,
  ) -> Result<Self> {
    let credentials = parse_google_cloud_credentials(&raw_credentials)?;

    Ok(Self {
      client: create_client(None),
      config: config.clone(),
      provider_id,
      provider_name,
      access_token: credentials.token,
      project_id: credentials.project_id,
      is_antigravity,
      base_url: config.base_url.clone(),
    })
  }

  fn endpoints(&self) -> Vec<String> {
    if let Some(base_url) = &self.base_url {
      return vec![base_url.clone()];
    }
    if self.is_antigravity {
      return vec![
        ANTIGRAVITY_DAILY_ENDPOINT.to_string(),
        DEFAULT_ENDPOINT.to_string(),
      ];
    }
    vec![DEFAULT_ENDPOINT.to_string()]
  }

  fn default_models_static(&self) -> &'static [&'static str] {
    if self.is_antigravity {
      return GOOGLE_ANTIGRAVITY_MODELS;
    }
    GOOGLE_GEMINI_CLI_MODELS
  }

  fn request_headers(&self, model: &str, access_token: &str) -> Result<reqwest::header::HeaderMap> {
    use reqwest::header::HeaderMap;
    use reqwest::header::HeaderName;
    use reqwest::header::HeaderValue;
    let mut headers = HeaderMap::new();
    headers.insert(
      reqwest::header::AUTHORIZATION,
      HeaderValue::from_str(&format!("Bearer {access_token}"))
        .map_err(|e| ModelError::AuthError(format!("invalid access token header: {e}")))?,
    );
    headers.insert(
      reqwest::header::CONTENT_TYPE,
      HeaderValue::from_static("application/json"),
    );
    headers.insert(
      reqwest::header::ACCEPT,
      HeaderValue::from_static("text/event-stream"),
    );
    if self.is_antigravity {
      let version = std::env::var("PI_AI_ANTIGRAVITY_VERSION")
        .unwrap_or_else(|_| DEFAULT_ANTIGRAVITY_VERSION.to_string());
      headers.insert(
        reqwest::header::USER_AGENT,
        HeaderValue::from_str(&format!("antigravity/{version} darwin/arm64"))
          .map_err(|e| ModelError::AuthError(format!("invalid antigravity user-agent: {e}")))?,
      );
      headers.insert(
        HeaderName::from_static("x-goog-api-client"),
        HeaderValue::from_static("google-cloud-sdk vscode_cloudshelleditor/0.1"),
      );
    } else {
      headers.insert(
        reqwest::header::USER_AGENT,
        HeaderValue::from_static("google-cloud-sdk vscode_cloudshelleditor/0.1"),
      );
      headers.insert(
        HeaderName::from_static("x-goog-api-client"),
        HeaderValue::from_static("gl-node/22.17.0"),
      );
    }
    headers.insert(
      HeaderName::from_static("client-metadata"),
      HeaderValue::from_static(
        r#"{"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}"#,
      ),
    );
    if is_claude_thinking_model(model) {
      headers.insert(
        HeaderName::from_static("anthropic-beta"),
        HeaderValue::from_static(CLAUDE_THINKING_BETA_HEADER),
      );
    }
    Ok(headers)
  }

  fn build_request(&self, request: &ChatRequest) -> CloudCodeAssistRequest {
    CloudCodeAssistRequest {
      project: self.project_id.clone(),
      model: request.model.clone(),
      request: build_cloud_code_assist_payload(request, self.is_antigravity),
      request_type: self.is_antigravity.then_some("agent".to_string()),
      user_agent: Some(if self.is_antigravity {
        "antigravity".to_string()
      } else {
        "cokra".to_string()
      }),
      request_id: Some(format!(
        "{}-{}-{}",
        if self.is_antigravity {
          "agent"
        } else {
          "cokra"
        },
        chrono::Utc::now().timestamp_millis(),
        uuid::Uuid::new_v4().simple()
      )),
    }
  }

  async fn stream_request(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>> {
    let client = self.client.clone();
    let request_model = request.model.clone();
    let request_headers = self.request_headers(&request_model, &self.access_token)?;
    let endpoints = self.endpoints();
    let body = self.build_request(&request);
    let request_body = serde_json::to_string(&body)
      .map_err(|e| ModelError::InvalidRequest(format!("invalid cloud code request: {e}")))?;
    let mut response = None;
    let mut last_err = None;

    for attempt in 0..=MAX_RETRIES {
      let endpoint = endpoints[(attempt as usize).min(endpoints.len().saturating_sub(1))].clone();
      let request_url = format!(
        "{}/v1internal:streamGenerateContent?alt=sse",
        endpoint.trim_end_matches('/')
      );
      match client
        .post(&request_url)
        .headers(request_headers.clone())
        .body(request_body.clone())
        .send()
        .await
      {
        Ok(resp) if resp.status().is_success() => {
          response = Some(resp);
          break;
        }
        Ok(resp) => {
          let status = resp.status();
          let error_text = resp.text().await.unwrap_or_default();
          if attempt < MAX_RETRIES && is_retryable_error(status.as_u16(), &error_text) {
            let delay_ms =
              extract_retry_delay(&error_text, None).unwrap_or(BASE_DELAY_MS * 2_u64.pow(attempt));
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            continue;
          }
          last_err = Some(ModelError::ApiError(format!(
            "Cloud Code Assist API error ({status}): {}",
            extract_error_message(&error_text)
          )));
        }
        Err(err) => {
          if attempt < MAX_RETRIES {
            let delay_ms = BASE_DELAY_MS * 2_u64.pow(attempt);
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            continue;
          }
          last_err = Some(ModelError::NetworkError(err));
        }
      }
    }

    let Some(response) = response else {
      return Err(
        last_err
          .unwrap_or_else(|| ModelError::ApiError("failed to get cloud code response".to_string())),
      );
    };

    let retry_endpoint = endpoints
      .first()
      .cloned()
      .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());

    let stream = async_stream::stream! {
      let mut emitted_stop = false;
      let mut empty_retry = 0;
      let mut active_response = response;

      loop {
        let mut saw_content = false;
        let mut byte_stream = active_response.bytes_stream();
        let mut buffer = String::new();

        while let Some(item) = byte_stream.next().await {
          match item {
            Ok(bytes) => {
              buffer.push_str(&String::from_utf8_lossy(&bytes));
              while let Some(idx) = buffer.find('\n') {
                let mut line = buffer[..idx].to_string();
                buffer.drain(..=idx);
                line = line.trim().to_string();
                if !line.starts_with("data:") {
                  continue;
                }
                let payload = line.trim_start_matches("data:").trim();
                if payload.is_empty() {
                  continue;
                }
                let chunk = match serde_json::from_str::<CloudCodeAssistResponseChunk>(payload) {
                  Ok(chunk) => chunk,
                  Err(err) => {
                    yield Err(ModelError::StreamError(format!("invalid Cloud Code Assist stream chunk: {err}")));
                    continue;
                  }
                };
                if let Some(response) = chunk.response {
                  if let Some(candidate) = response.candidates.first() {
                    if let Some(content) = &candidate.content {
                      if let Some(parts) = &content.parts {
                        for part in parts {
                          if let Some(text) = &part.text {
                            if !text.is_empty() {
                              saw_content = true;
                              yield Ok(Chunk::Content {
                                delta: ContentDelta { text: text.clone() },
                              });
                            }
                          }
                          if let Some(function_call) = &part.function_call {
                            saw_content = true;
                            let id = function_call.id.clone().unwrap_or_else(|| {
                              format!("{}_{}", function_call.name, uuid::Uuid::new_v4().simple())
                            });
                            let arguments = serde_json::to_string(&function_call.args).unwrap_or_else(|_| "{}".to_string());
                            // Extract thought_signature from response part for Gemini 3 compatibility
                            let thought_signature = part.thought_signature.clone();
                            yield Ok(Chunk::ToolCall {
                              delta: ToolCallDelta {
                                id: Some(id),
                                name: Some(function_call.name.clone()),
                                arguments: Some(arguments),
                                thought_signature,
                              },
                            });
                          }
                        }
                      }
                    }
                    if candidate.finish_reason.is_some() && !emitted_stop {
                      emitted_stop = true;
                      yield Ok(Chunk::MessageStop);
                    }
                  }
                }
              }
            }
            Err(err) => {
              yield Err(ModelError::StreamError(err.to_string()));
              return;
            }
          }
        }

        if saw_content || empty_retry >= MAX_EMPTY_STREAM_RETRIES {
          if !emitted_stop {
            yield Ok(Chunk::MessageStop);
          }
          break;
        }

        empty_retry += 1;
        tokio::time::sleep(std::time::Duration::from_millis(EMPTY_STREAM_BASE_DELAY_MS * 2_u64.pow(empty_retry - 1))).await;
        let retry_url = format!(
          "{}/v1internal:streamGenerateContent?alt=sse",
          retry_endpoint.trim_end_matches('/'),
        );
        active_response = match client
          .post(&retry_url)
          .headers(request_headers.clone())
          .body(request_body.clone())
          .send()
          .await
        {
          Ok(resp) if resp.status().is_success() => resp,
          Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            yield Err(ModelError::ApiError(format!("Cloud Code Assist API error ({status}): {body}")));
            return;
          }
          Err(err) => {
            yield Err(ModelError::NetworkError(err));
            return;
          }
        };
      }
    };

    Ok(Box::pin(stream))
  }
}

#[async_trait]
impl ModelProvider for GoogleCloudCodeProvider {
  fn provider_id(&self) -> &'static str {
    self.provider_id
  }

  fn provider_name(&self) -> &'static str {
    self.provider_name
  }

  fn required_env_vars(&self) -> Vec<&'static str> {
    Vec::new()
  }

  fn default_models(&self) -> Vec<&'static str> {
    self.default_models_static().to_vec()
  }

  async fn chat_completion(&self, request: ChatRequest) -> Result<ChatResponse> {
    let model = request.model.clone();
    let mut stream = self.stream_request(request).await?;
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    while let Some(item) = stream.next().await {
      match item? {
        Chunk::Content { delta } => {
          content.push_str(&delta.text);
        }
        Chunk::ToolCall { delta } => {
          tool_calls.push(ToolCall {
            id: delta.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            call_type: "function".to_string(),
            function: ToolCallFunction {
              name: delta.name.unwrap_or_default(),
              arguments: delta.arguments.unwrap_or_else(|| "{}".to_string()),
            },
            // Preserve thought_signature for Gemini 3 multi-turn function calling
            provider_meta: delta.thought_signature.map(|sig| ToolCallProviderMeta {
              thought_signature: Some(sig),
            }),
          });
        }
        Chunk::MessageStop
        | Chunk::Unknown
        | Chunk::MessageStart { .. }
        | Chunk::MessageDelta { .. } => {}
      }
    }

    Ok(ChatResponse {
      id: uuid::Uuid::new_v4().to_string(),
      object_type: "chat.completion".to_string(),
      created: chrono::Utc::now().timestamp() as u64,
      model,
      choices: vec![Choice {
        index: 0,
        message: ChoiceMessage {
          role: "assistant".to_string(),
          content: (!content.is_empty()).then_some(content),
          tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
        },
        finish_reason: Some("stop".to_string()),
      }],
      usage: Usage::default(),
      extra: HashMap::new(),
    })
  }

  async fn chat_completion_stream(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>> {
    self.stream_request(request).await
  }

  async fn list_models(&self) -> Result<ListModelsResponse> {
    Ok(ListModelsResponse {
      object_type: "list".to_string(),
      data: self
        .default_models_static()
        .iter()
        .map(|id| ModelInfo {
          id: (*id).to_string(),
          object_type: "model".to_string(),
          created: 0,
          owned_by: Some(self.provider_id.to_string()),
        })
        .collect(),
    })
  }

  async fn validate_auth(&self) -> Result<()> {
    if self.project_id.is_empty() {
      return Err(ModelError::AuthError(
        "Google Cloud Code Assist credentials are incomplete".to_string(),
      ));
    }
    if self.access_token.is_empty() {
      return Err(ModelError::AuthError(
        "Google Cloud Code Assist access token is missing".to_string(),
      ));
    }
    Ok(())
  }

  fn client(&self) -> &Client {
    &self.client
  }

  fn config(&self) -> &ProviderConfig {
    &self.config
  }
}

#[derive(Debug, Clone, Deserialize)]
struct GoogleCloudCredentials {
  token: String,
  #[serde(rename = "projectId")]
  project_id: String,
}

fn parse_google_cloud_credentials(raw: &str) -> Result<GoogleCloudCredentials> {
  serde_json::from_str::<GoogleCloudCredentials>(raw).map_err(|e| {
    ModelError::AuthError(format!(
      "invalid Google Cloud Code Assist credentials, please reconnect: {e}"
    ))
  })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudCodeAssistRequest {
  project: String,
  model: String,
  request: CloudCodeAssistRequestPayload,
  #[serde(skip_serializing_if = "Option::is_none")]
  request_type: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  user_agent: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudCodeAssistRequestPayload {
  contents: Vec<CloudCodeAssistContent>,
  #[serde(skip_serializing_if = "Option::is_none")]
  system_instruction: Option<CloudCodeAssistSystemInstruction>,
  #[serde(skip_serializing_if = "Option::is_none")]
  generation_config: Option<CloudCodeAssistGenerationConfig>,
  #[serde(skip_serializing_if = "Option::is_none")]
  tools: Option<Vec<CloudCodeAssistToolset>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  tool_config: Option<CloudCodeAssistToolConfig>,
}

#[derive(Debug, Clone, Serialize)]
struct CloudCodeAssistContent {
  role: String,
  parts: Vec<CloudCodeAssistPart>,
}

#[derive(Debug, Clone, Serialize)]
struct CloudCodeAssistSystemInstruction {
  #[serde(skip_serializing_if = "Option::is_none")]
  role: Option<String>,
  parts: Vec<CloudCodeAssistSystemPart>,
}

#[derive(Debug, Clone, Serialize)]
struct CloudCodeAssistSystemPart {
  text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudCodeAssistPart {
  #[serde(skip_serializing_if = "Option::is_none")]
  text: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  thought: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  function_call: Option<CloudCodeAssistFunctionCall>,
  #[serde(skip_serializing_if = "Option::is_none")]
  function_response: Option<CloudCodeAssistFunctionResponse>,
  /// Google Gemini 3 thought signature - required for multi-turn function calling
  /// Must be passed back exactly as received from the model response
  #[serde(skip_serializing_if = "Option::is_none")]
  thought_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CloudCodeAssistFunctionCall {
  name: String,
  args: serde_json::Value,
  #[serde(skip_serializing_if = "Option::is_none")]
  id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CloudCodeAssistFunctionResponse {
  name: String,
  response: serde_json::Value,
  #[serde(skip_serializing_if = "Option::is_none")]
  id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudCodeAssistGenerationConfig {
  #[serde(skip_serializing_if = "Option::is_none")]
  max_output_tokens: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  temperature: Option<f32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  top_p: Option<f32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  thinking_config: Option<CloudCodeAssistThinkingConfig>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudCodeAssistThinkingConfig {
  include_thoughts: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  thinking_budget: Option<u32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  thinking_level: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CloudCodeAssistToolset {
  #[serde(rename = "functionDeclarations")]
  function_declarations: Vec<CloudCodeAssistFunctionDeclaration>,
}

#[derive(Debug, Clone, Serialize)]
struct CloudCodeAssistFunctionDeclaration {
  name: String,
  description: String,
  #[serde(
    rename = "parametersJsonSchema",
    skip_serializing_if = "Option::is_none"
  )]
  parameters_json_schema: Option<serde_json::Value>,
  #[serde(skip_serializing_if = "Option::is_none")]
  parameters: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudCodeAssistToolConfig {
  function_calling_config: CloudCodeAssistFunctionCallingConfig,
}

#[derive(Debug, Clone, Serialize)]
struct CloudCodeAssistFunctionCallingConfig {
  mode: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudCodeAssistResponseChunk {
  response: Option<CloudCodeAssistResponse>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudCodeAssistResponse {
  candidates: Vec<CloudCodeAssistCandidate>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudCodeAssistCandidate {
  content: Option<CloudCodeAssistResponseContent>,
  finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CloudCodeAssistResponseContent {
  parts: Option<Vec<CloudCodeAssistResponsePart>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudCodeAssistResponsePart {
  text: Option<String>,
  thought: Option<bool>,
  function_call: Option<CloudCodeAssistResponseFunctionCall>,
  /// Google Gemini 3 thought signature - required for multi-turn function calling
  #[serde(rename = "thoughtSignature")]
  thought_signature: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CloudCodeAssistResponseFunctionCall {
  name: String,
  args: serde_json::Value,
  id: Option<String>,
}

fn build_cloud_code_assist_payload(
  request: &ChatRequest,
  is_antigravity: bool,
) -> CloudCodeAssistRequestPayload {
  let mut tool_names = HashMap::new();
  let mut contents = Vec::new();
  let mut system_parts = Vec::new();

  for message in &request.messages {
    match message {
      Message::System(text) => {
        if !text.is_empty() {
          system_parts.push(CloudCodeAssistSystemPart { text: text.clone() });
        }
      }
      Message::User(text) => {
        contents.push(CloudCodeAssistContent {
          role: "user".to_string(),
          parts: vec![CloudCodeAssistPart {
            text: Some(text.clone()),
            thought: None,
            function_call: None,
            function_response: None,
            thought_signature: None,
          }],
        });
      }
      Message::Assistant {
        content,
        tool_calls,
      } => {
        let mut parts = Vec::new();
        if let Some(content) = content
          && !content.trim().is_empty()
        {
          parts.push(CloudCodeAssistPart {
            text: Some(content.clone()),
            thought: None,
            function_call: None,
            function_response: None,
            thought_signature: None,
          });
        }
        if let Some(tool_calls) = tool_calls {
          for tool_call in tool_calls {
            let args = serde_json::from_str(&tool_call.function.arguments)
              .unwrap_or_else(|_| serde_json::json!({ "raw": tool_call.function.arguments }));
            tool_names.insert(tool_call.id.clone(), tool_call.function.name.clone());
            // Extract thought_signature from provider_meta for Gemini 3 compatibility
            let thought_signature = tool_call
              .provider_meta
              .as_ref()
              .and_then(|m| m.thought_signature.clone());
            parts.push(CloudCodeAssistPart {
              text: None,
              thought: None,
              function_call: Some(CloudCodeAssistFunctionCall {
                name: tool_call.function.name.clone(),
                args,
                id: Some(tool_call.id.clone()),
              }),
              function_response: None,
              thought_signature,
            });
          }
        }
        if !parts.is_empty() {
          contents.push(CloudCodeAssistContent {
            role: "model".to_string(),
            parts,
          });
        }
      }
      Message::Tool {
        tool_call_id,
        content,
      } => {
        let tool_name = tool_names
          .get(tool_call_id)
          .cloned()
          .unwrap_or_else(|| "tool".to_string());
        contents.push(CloudCodeAssistContent {
          role: "user".to_string(),
          parts: vec![CloudCodeAssistPart {
            text: None,
            thought: None,
            function_call: None,
            function_response: Some(CloudCodeAssistFunctionResponse {
              name: tool_name,
              response: serde_json::json!({ "output": content }),
              id: Some(tool_call_id.clone()),
            }),
            thought_signature: None,
          }],
        });
      }
    }
  }

  let mut instruction_parts = if is_antigravity {
    vec![
      CloudCodeAssistSystemPart {
        text: ANTIGRAVITY_SYSTEM_INSTRUCTION.to_string(),
      },
      CloudCodeAssistSystemPart {
        text: format!(
          "Please ignore following [ignore]{}[/ignore]",
          ANTIGRAVITY_SYSTEM_INSTRUCTION
        ),
      },
    ]
  } else {
    Vec::new()
  };
  instruction_parts.extend(system_parts);

  let generation_config =
    if request.temperature.is_some() || request.max_tokens.is_some() || request.top_p.is_some() {
      Some(CloudCodeAssistGenerationConfig {
        max_output_tokens: request.max_tokens,
        temperature: request.temperature,
        top_p: request.top_p,
        thinking_config: infer_thinking_config(&request.model),
      })
    } else {
      infer_thinking_config(&request.model).map(|thinking_config| CloudCodeAssistGenerationConfig {
        max_output_tokens: request.max_tokens,
        temperature: request.temperature,
        top_p: request.top_p,
        thinking_config: Some(thinking_config),
      })
    };

  CloudCodeAssistRequestPayload {
    contents,
    system_instruction: (!instruction_parts.is_empty()).then_some(
      CloudCodeAssistSystemInstruction {
        role: is_antigravity.then_some("user".to_string()),
        parts: instruction_parts,
      },
    ),
    generation_config,
    tools: convert_tools_for_cloud_code(
      request.tools.as_ref(),
      request.model.starts_with("claude-"),
    ),
    tool_config: request
      .tool_choice
      .as_ref()
      .map(|tool_choice| CloudCodeAssistToolConfig {
        function_calling_config: CloudCodeAssistFunctionCallingConfig {
          mode: map_tool_choice(tool_choice),
        },
      }),
  }
}

fn convert_tools_for_cloud_code(
  tools: Option<&Vec<Tool>>,
  use_parameters: bool,
) -> Option<Vec<CloudCodeAssistToolset>> {
  let tools = tools?;
  if tools.is_empty() {
    return None;
  }
  Some(vec![CloudCodeAssistToolset {
    function_declarations: tools
      .iter()
      .filter_map(|tool| {
        let function = tool.function.as_ref()?;
        Some(CloudCodeAssistFunctionDeclaration {
          name: function.name.clone(),
          description: function.description.clone(),
          parameters_json_schema: (!use_parameters).then_some(function.parameters.clone()),
          parameters: use_parameters.then_some(function.parameters.clone()),
        })
      })
      .collect(),
  }])
}

fn map_tool_choice(choice: &str) -> String {
  match choice {
    "none" => "NONE".to_string(),
    "required" | "any" => "ANY".to_string(),
    _ => "AUTO".to_string(),
  }
}

fn infer_thinking_config(model: &str) -> Option<CloudCodeAssistThinkingConfig> {
  let normalized = model.to_lowercase();
  if normalized.contains("gemini-3-pro") {
    return Some(CloudCodeAssistThinkingConfig {
      include_thoughts: true,
      thinking_budget: None,
      thinking_level: Some("HIGH".to_string()),
    });
  }
  if normalized.contains("gemini-3-flash") {
    return Some(CloudCodeAssistThinkingConfig {
      include_thoughts: true,
      thinking_budget: None,
      thinking_level: Some("MEDIUM".to_string()),
    });
  }
  if normalized.contains("gemini-2.5") || is_claude_thinking_model(model) {
    return Some(CloudCodeAssistThinkingConfig {
      include_thoughts: true,
      thinking_budget: Some(2048),
      thinking_level: None,
    });
  }
  None
}

fn is_claude_thinking_model(model: &str) -> bool {
  let normalized = model.to_lowercase();
  normalized.contains("claude") && normalized.contains("thinking")
}

fn is_retryable_error(status: u16, error_text: &str) -> bool {
  if matches!(status, 429 | 500 | 502 | 503 | 504) {
    return true;
  }
  let normalized = error_text.to_lowercase();
  normalized.contains("resource exhausted")
    || normalized.contains("rate limit")
    || normalized.contains("overloaded")
    || normalized.contains("service unavailable")
    || normalized.contains("other side closed")
}

fn extract_retry_delay(
  error_text: &str,
  _headers: Option<&reqwest::header::HeaderMap>,
) -> Option<u64> {
  let marker = "Please retry in ";
  let start = error_text.find(marker)? + marker.len();
  let rest = &error_text[start..];
  let mut number = String::new();
  for ch in rest.chars() {
    if ch.is_ascii_digit() || ch == '.' {
      number.push(ch);
      continue;
    }
    break;
  }
  if number.is_empty() {
    return None;
  }
  let value = number.parse::<f64>().ok()?;
  let unit = rest[number.len()..].trim_start();
  if unit.starts_with("ms") {
    return Some(value.ceil() as u64 + 1000);
  }
  if unit.starts_with('s') {
    return Some((value * 1000.0).ceil() as u64 + 1000);
  }
  None
}

fn extract_error_message(error_text: &str) -> String {
  serde_json::from_str::<serde_json::Value>(error_text)
    .ok()
    .and_then(|json| {
      json
        .get("error")?
        .get("message")?
        .as_str()
        .map(ToString::to_string)
    })
    .unwrap_or_else(|| error_text.to_string())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_google_cloud_credentials() {
    let creds =
      parse_google_cloud_credentials(r#"{"token":"tok","projectId":"proj"}"#).expect("credentials");
    assert_eq!(creds.token, "tok");
    assert_eq!(creds.project_id, "proj");
  }

  #[test]
  fn antigravity_request_includes_request_type_and_system_instruction() {
    let provider = GoogleCloudCodeProvider::new_antigravity(
      r#"{"token":"tok","projectId":"proj"}"#.to_string(),
      ProviderConfig {
        provider_id: "google-antigravity".to_string(),
        ..Default::default()
      },
    )
    .expect("provider");

    let body = provider.build_request(&ChatRequest {
      model: "gemini-3-pro-high".to_string(),
      messages: vec![
        Message::System("system".to_string()),
        Message::User("hello".to_string()),
      ],
      ..Default::default()
    });

    assert_eq!(body.project, "proj");
    assert_eq!(body.request_type.as_deref(), Some("agent"));
    assert!(body.request.system_instruction.is_some());
    assert_eq!(body.request.contents.len(), 1);
  }
}
