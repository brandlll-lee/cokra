//! GitHub Copilot Provider
//!
//! Support for GitHub Copilot models (uses OAuth or token-based auth)

use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;
use reqwest::RequestBuilder;
use serde::Deserialize;
use serde::Serialize;
use std::pin::Pin;

use super::super::auth::Credentials;
use super::super::auth::StoredCredentials;
use super::super::error::ModelError;
use super::super::error::Result;
use super::super::provider::ModelProvider;
use super::super::transform::ProviderRuntimeTransform;
use super::super::types::ChatRequest;
use super::super::types::ChatResponse;
use super::super::types::Chunk;
use super::super::types::ListModelsResponse;
use super::super::types::ProviderConfig;
use super::build_openai_request;
use super::create_client;
use super::create_response_stream;
use super::parse_openai_response;

/// GitHub Copilot provider
pub struct GitHubCopilotProvider {
  client: Client,
  config: ProviderConfig,
  token: String,
  base_url: String,
  use_responses_api: bool,
  oauth_mode: bool,
}

// GitHub Copilot headers (pi-mono parity).
const COPILOT_HEADERS: [(&str, &str); 3] = [
  ("Editor-Version", "vscode/1.107.0"),
  ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
  ("Copilot-Integration-Id", "vscode-chat"),
];

impl GitHubCopilotProvider {
  /// Create a new GitHub Copilot provider with a token
  pub fn new(token: String, config: ProviderConfig) -> Self {
    let base_url = provider_base_url(&config, None);

    let client = create_client(config.timeout);

    Self {
      client,
      config,
      token,
      base_url,
      use_responses_api: true, // Use Responses API by default
      oauth_mode: false,
    }
  }

  pub fn new_oauth(stored: &StoredCredentials, config: ProviderConfig) -> Result<Self> {
    let Credentials::OAuth {
      access_token,
      enterprise_url,
      ..
    } = &stored.credentials
    else {
      return Err(ModelError::AuthError(
        "GitHub Copilot OAuth provider requires OAuth credentials".to_string(),
      ));
    };

    let base_url = stored
      .metadata
      .get("base_url")
      .and_then(serde_json::Value::as_str)
      .map(str::trim)
      .filter(|value| !value.is_empty())
      .map(ToString::to_string)
      .or_else(|| {
        super::super::oauth_connect::get_github_base_url_from_token(access_token)
      })
      .unwrap_or_else(|| provider_base_url(&config, enterprise_url.as_deref()));

    Ok(Self {
      client: create_client(config.timeout),
      base_url,
      config,
      token: access_token.clone(),
      use_responses_api: true,
      oauth_mode: true,
    })
  }

  /// Get the API endpoint URL
  fn endpoint(&self, path: &str) -> String {
    format!("{}/{}", self.base_url.trim_end_matches('/'), path)
  }

  /// Check if we should use the Responses API for this model
  fn should_use_responses_api(&self, model: &str) -> bool {
    // Use Responses API for o1 models
    self.use_responses_api && (model.starts_with("o1-") || model.contains("o1"))
  }

  /// Build authorization header
  fn auth_header(&self) -> String {
    format!("Bearer {}", self.token)
  }

  fn apply_headers(&self, request: RequestBuilder, input: Option<&ChatRequest>) -> RequestBuilder {
    let mut request = request.header("Authorization", self.auth_header()).header(
      "User-Agent",
      if self.oauth_mode {
        "GitHubCopilotChat/0.35.0"
      } else {
        "cokra/github-copilot"
      },
    );

    for (key, value) in COPILOT_HEADERS {
      request = request.header(key, value);
    }

    if self.oauth_mode {
      let initiator = input
        .and_then(|request| request.messages.last())
        .map(|message| match message {
          crate::model::types::Message::User(_) => "user",
          _ => "agent",
        })
        .unwrap_or("user");
      request = request
        .header("x-initiator", initiator)
        .header("Openai-Intent", "conversation-edits");
    }

    if input.is_some_and(|request| request.model.contains("claude")) {
      request = request.header("anthropic-beta", "interleaved-thinking-2025-05-14");
    }

    for (key, value) in &self.config.headers {
      request = request.header(key, value);
    }

    request
  }
}

/// Default models for GitHub Copilot
pub const GITHUB_MODELS: &[&str] = &[
  // OpenAI models via Copilot
  "gpt-4o",
  "gpt-4o-mini",
  "gpt-4-turbo",
  "gpt-3.5-turbo",
  // O-series reasoning models
  "o1-2024-12-17",
  "o1-mini-2024-09-12",
  "o1-preview-2024-09-12",
  // Claude models (if available via Copilot)
  "claude-sonnet-4-20250514",
  "claude-3-5-sonnet-20241022",
];

// GitHub Copilot API types

#[derive(Debug, Serialize, Clone)]
struct CopilotMessage {
  role: String,
  content: String,
}

// Responses API types

#[derive(Debug, Serialize)]
struct ResponsesApiRequest {
  messages: Vec<CopilotMessage>,
  model: String,
  stream: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  store: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  temperature: Option<f32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  max_tokens: Option<u32>,
}

#[async_trait]
impl ModelProvider for GitHubCopilotProvider {
  fn provider_id(&self) -> &'static str {
    "github"
  }

  fn provider_name(&self) -> &'static str {
    "GitHub Copilot"
  }

  fn required_env_vars(&self) -> Vec<&'static str> {
    vec!["GITHUB_TOKEN", "GITHUB_COPILOT_TOKEN"]
  }

  fn default_models(&self) -> Vec<&'static str> {
    GITHUB_MODELS.to_vec()
  }

  async fn chat_completion(&self, request: ChatRequest) -> Result<ChatResponse> {
    let request = ProviderRuntimeTransform::from_config(&self.config).normalize_request(request);
    // Choose API based on model
    let use_responses = self.should_use_responses_api(&request.model);

    if use_responses {
      // Responses API does not currently support the full OpenAI-compatible tool schema in
      // GitHub Copilot, so we keep a simplified text-only message format here.
      let messages: Vec<CopilotMessage> = request
        .messages
        .iter()
        .map(|m| match m {
          crate::model::types::Message::System(s) => CopilotMessage {
            role: "system".to_string(),
            content: s.clone(),
          },
          crate::model::types::Message::User(s) => CopilotMessage {
            role: "user".to_string(),
            content: s.clone(),
          },
          crate::model::types::Message::Assistant { content, .. } => CopilotMessage {
            role: "assistant".to_string(),
            content: content.clone().unwrap_or_default(),
          },
          crate::model::types::Message::Tool {
            tool_call_id,
            content,
          } => CopilotMessage {
            role: "user".to_string(),
            content: format!("[Tool Result for {}]: {}", tool_call_id, content),
          },
        })
        .collect();
      self.chat_completion_responses(request, messages).await
    } else {
      self.chat_completion_chat(request).await
    }
  }

  async fn chat_completion_stream(
    &self,
    request: ChatRequest,
  ) -> Result<Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>> {
    let request = ProviderRuntimeTransform::from_config(&self.config).normalize_request(request);
    let url = self.endpoint("chat/completions");
    let model = request.model.clone();
    let header_input = request.clone();
    let body = build_openai_request(request, &model);

    let response = self
      .apply_headers(self.client.post(&url), Some(&header_input))
      .header("Content-Type", "application/json")
      .json(&body)
      .send()
      .await
      .map_err(ModelError::NetworkError)?;

    Ok(create_response_stream(response))
  }

  async fn list_models(&self) -> Result<ListModelsResponse> {
    // Prefer live list from Copilot API (opencode parity: don't ship a stale hardcoded list).
    let candidates = ["models", "v1/models"];
    for path in candidates {
      let url = self.endpoint(path);
      let response = match self
        .apply_headers(self.client.get(&url), None)
        .header("Accept", "application/json")
        .send()
        .await
      {
        Ok(resp) => resp,
        Err(_) => continue,
      };

      if response.status().is_success() {
        // Try strict ListModelsResponse shape first.
        if let Ok(parsed) = response.json::<ListModelsResponse>().await {
          if !parsed.data.is_empty() {
            return Ok(parsed);
          }
        }
      }
    }

    // Fallback: static list of known Copilot models.
    Ok(ListModelsResponse {
      object_type: "list".to_string(),
      data: GITHUB_MODELS
        .iter()
        .map(|&id| crate::model::types::ModelInfo {
          id: id.to_string(),
          object_type: "model".to_string(),
          created: 1704067200,
          owned_by: Some("github".to_string()),
        })
        .collect(),
    })
  }

  async fn validate_auth(&self) -> Result<()> {
    // Try a simple request to validate
    let url = self.endpoint("chat/completions");

    let request = ChatRequest {
      model: "gpt-4o-mini".to_string(),
      messages: vec![crate::model::types::Message::User("Hi".to_string())],
      stream: false,
      temperature: Some(0.0),
      ..Default::default()
    };
    let body = build_openai_request(request.clone(), "gpt-4o-mini");

    let response = self
      .apply_headers(self.client.post(&url), Some(&request))
      .header("Content-Type", "application/json")
      .json(&body)
      .send()
      .await
      .map_err(ModelError::NetworkError)?;

    if response.status().is_success() {
      Ok(())
    } else if response.status().as_u16() == 401 {
      Err(ModelError::AuthError("Invalid GitHub token".to_string()))
    } else {
      Err(ModelError::AuthError("Authentication failed".to_string()))
    }
  }

  fn client(&self) -> &Client {
    &self.client
  }

  fn config(&self) -> &ProviderConfig {
    &self.config
  }
}

impl GitHubCopilotProvider {
  /// Chat completion using the Chat API
  async fn chat_completion_chat(&self, request: ChatRequest) -> Result<ChatResponse> {
    let url = self.endpoint("chat/completions");
    let model = request.model.clone();
    let header_input = request.clone();
    let body = build_openai_request(request, &model);

    let response = self
      .apply_headers(self.client.post(&url), Some(&header_input))
      .header("Content-Type", "application/json")
      .json(&body)
      .send()
      .await
      .map_err(ModelError::NetworkError)?;

    if !response.status().is_success() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(ModelError::ApiError(format!("HTTP {}: {}", status, body)));
    }

    let text = response.text().await.unwrap_or_default();
    Ok(parse_openai_response(&text)?)
  }

  /// Chat completion using the Responses API (for o1 models)
  async fn chat_completion_responses(
    &self,
    request: ChatRequest,
    messages: Vec<CopilotMessage>,
  ) -> Result<ChatResponse> {
    let url = format!("{}/responses", self.base_url);
    let model = request.model.clone();

    let body = ResponsesApiRequest {
      messages,
      model: model.clone(),
      stream: false,
      store: ProviderRuntimeTransform::from_config(&self.config).store_flag(),
      temperature: request.temperature,
      max_tokens: request.max_tokens,
    };

    let response = self
      .apply_headers(self.client.post(&url), Some(&request))
      .header("Content-Type", "application/json")
      .json(&body)
      .send()
      .await
      .map_err(ModelError::NetworkError)?;

    if !response.status().is_success() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(ModelError::ApiError(format!("HTTP {}: {}", status, body)));
    }

    #[derive(Deserialize)]
    struct ResponsesApiResponse {
      id: String,
      body: ResponsesApiBody,
    }

    #[derive(Deserialize)]
    struct ResponsesApiBody {
      content: String,
    }

    let resp: ResponsesApiResponse = response.json().await?;

    Ok(ChatResponse {
      id: resp.id,
      object_type: "chat.completion".to_string(),
      created: chrono::Utc::now().timestamp() as u64,
      model,
      choices: vec![crate::model::types::Choice {
        index: 0,
        message: crate::model::types::ChoiceMessage {
          role: "assistant".to_string(),
          content: Some(resp.body.content),
          tool_calls: None,
        },
        finish_reason: Some("stop".to_string()),
      }],
      usage: crate::model::types::Usage::default(),
      extra: Default::default(),
    })
  }
}

fn provider_base_url(config: &ProviderConfig, enterprise_url: Option<&str>) -> String {
  if let Some(base_url) = config.base_url.clone() {
    return base_url;
  }

  if let Some(enterprise_url) = enterprise_url {
    let normalized = enterprise_url
      .trim()
      .trim_start_matches("https://")
      .trim_start_matches("http://")
      .trim_end_matches('/');
    if !normalized.is_empty() {
      return format!("https://copilot-api.{normalized}");
    }
  }

  "https://api.githubcopilot.com".to_string()
}
