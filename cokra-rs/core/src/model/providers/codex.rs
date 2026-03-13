use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use futures::Stream;
use reqwest::Client;
use reqwest::RequestBuilder;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;

use super::super::auth::AuthManager;
use super::super::auth::Credentials;
use super::super::auth::StoredCredentials;
use super::super::error::ModelError;
use super::super::error::Result;
use super::super::provider::ModelProvider;
use super::super::provider::ResponseEventStream;
use super::super::streaming::create_openai_responses_event_stream;
use super::super::streaming::response_event_stream_to_chunk_stream;
use super::super::transform::ProviderRuntimeTransform;
use super::super::types::ChatRequest;
use super::super::types::ChatResponse;
use super::super::types::Choice;
use super::super::types::ChoiceMessage;
use super::super::types::Chunk;
use super::super::types::FunctionDefinition;
use super::super::types::ListModelsResponse;
use super::super::types::ModelInfo;
use super::super::types::ProviderConfig;
use super::super::types::Tool;
use super::super::types::ToolCall;
use super::super::types::Usage;
use super::create_client;

const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_API_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
const CODEX_ORIGINATOR: &str = "opencode";
const TOKEN_REFRESH_SKEW_SECS: u64 = 30;

pub const OPENAI_CODEX_MODELS: &[&str] = &[
  "gpt-5.3-codex",
  "gpt-5.2-codex",
  "gpt-5.1-codex",
  "gpt-5.1-codex-mini",
  "gpt-5.1-codex-max",
];

#[derive(Debug, Clone)]
struct OAuthState {
  access_token: String,
  refresh_token: String,
  expires_at: u64,
  account_id: Option<String>,
}

pub struct OpenAICodexProvider {
  client: Client,
  config: ProviderConfig,
  connect_source: String,
  state: Arc<RwLock<OAuthState>>,
}

impl OpenAICodexProvider {
  pub fn new(stored: &StoredCredentials, config: ProviderConfig) -> Result<Self> {
    let Credentials::OAuth {
      access_token,
      refresh_token,
      expires_at,
      account_id,
      ..
    } = &stored.credentials
    else {
      return Err(ModelError::AuthError(
        "OpenAI Codex provider requires OAuth credentials".to_string(),
      ));
    };

    Ok(Self {
      client: create_client(config.timeout),
      connect_source: stored.provider_id.clone(),
      config,
      state: Arc::new(RwLock::new(OAuthState {
        access_token: access_token.clone(),
        refresh_token: refresh_token.clone(),
        expires_at: *expires_at,
        account_id: account_id.clone().or_else(|| {
          stored
            .metadata
            .get("organization_id")
            .and_then(Value::as_str)
            .map(ToString::to_string)
        }),
      })),
    })
  }

  async fn ensure_access_token(&self) -> Result<OAuthState> {
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    {
      let state = self.state.read().await;
      if state.expires_at > now + TOKEN_REFRESH_SKEW_SECS {
        return Ok(state.clone());
      }
    }

    let refresh_token = {
      let state = self.state.read().await;
      state.refresh_token.clone()
    };
    let refreshed = self.refresh_access_token(&refresh_token).await?;
    let mut state = self.state.write().await;
    *state = refreshed.clone();
    Ok(refreshed)
  }

  async fn refresh_access_token(&self, refresh_token: &str) -> Result<OAuthState> {
    let response = reqwest::Client::new()
      .post(OPENAI_TOKEN_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .form(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", OPENAI_CLIENT_ID),
      ])
      .send()
      .await
      .map_err(ModelError::NetworkError)?;

    if !response.status().is_success() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(ModelError::AuthError(format!(
        "OpenAI Codex token refresh failed (HTTP {status}): {body}"
      )));
    }

    let token: RefreshTokenResponse = response.json().await?;
    let expires_at =
      chrono::Utc::now().timestamp().max(0) as u64 + token.expires_in.unwrap_or(3600);
    let account_id = token
      .id_token
      .as_deref()
      .and_then(extract_openai_account_id_from_jwt)
      .or_else(|| extract_openai_account_id_from_jwt(&token.access_token));

    let next = OAuthState {
      access_token: token.access_token.clone(),
      refresh_token: token
        .refresh_token
        .unwrap_or_else(|| refresh_token.to_string()),
      expires_at,
      account_id,
    };

    self.persist_refreshed_state(&next).await?;
    Ok(next)
  }

  async fn persist_refreshed_state(&self, next: &OAuthState) -> Result<()> {
    let auth = AuthManager::new()
      .map_err(|err| ModelError::AuthError(format!("failed to open auth manager: {err}")))?;
    let Some(mut stored) = auth
      .storage()
      .load(&self.connect_source)
      .await
      .map_err(|err| ModelError::AuthError(format!("failed to load stored auth: {err}")))?
    else {
      return Ok(());
    };

    stored.credentials = Credentials::OAuth {
      access_token: next.access_token.clone(),
      refresh_token: next.refresh_token.clone(),
      expires_at: next.expires_at,
      account_id: next.account_id.clone(),
      enterprise_url: None,
    };

    auth
      .storage()
      .save(stored)
      .await
      .map_err(|err| ModelError::AuthError(format!("failed to persist refreshed auth: {err}")))?;
    Ok(())
  }

  async fn authorized_request(&self, request: RequestBuilder) -> Result<RequestBuilder> {
    let state = self.ensure_access_token().await?;
    let mut request = request
      .header("Authorization", format!("Bearer {}", state.access_token))
      .header("originator", CODEX_ORIGINATOR);

    if let Some(account_id) = state.account_id {
      request = request.header("ChatGPT-Account-Id", account_id);
    }

    for (key, value) in &self.config.headers {
      request = request.header(key, value);
    }

    Ok(request)
  }

  fn build_responses_body(&self, request: ChatRequest) -> Value {
    // Tradeoff: normalize again at the provider boundary so direct provider
    // callers cannot bypass the opencode-aligned request policy in ModelClient.
    let request = ProviderRuntimeTransform::from_config(&self.config).normalize_request(request);
    let mut instructions = Vec::new();
    let mut input = Vec::<Value>::new();

    for message in request.messages {
      match message {
        super::super::types::Message::System(content) => {
          if !content.is_empty() {
            instructions.push(content);
          }
        }
        super::super::types::Message::User(content) => {
          input.push(serde_json::json!({
            "role": "user",
            "content": [{
              "type": "input_text",
              "text": content,
            }],
          }));
        }
        super::super::types::Message::Assistant {
          content,
          tool_calls,
        } => {
          if let Some(content) = content.filter(|content| !content.is_empty()) {
            input.push(serde_json::json!({
              "role": "assistant",
              "content": [{
                "type": "output_text",
                "text": content,
              }],
            }));
          }

          if let Some(tool_calls) = tool_calls {
            for call in tool_calls {
              input.push(serde_json::json!({
                "type": "function_call",
                "call_id": call.id,
                "name": call.function.name,
                "arguments": call.function.arguments,
              }));
            }
          }
        }
        super::super::types::Message::Tool {
          tool_call_id,
          content,
        } => {
          input.push(serde_json::json!({
            "type": "function_call_output",
            "call_id": tool_call_id,
            "output": content,
          }));
        }
      }
    }

    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), Value::String(request.model));
    body.insert("stream".to_string(), Value::Bool(true));
    if let Some(store) = ProviderRuntimeTransform::from_config(&self.config).store_flag() {
      body.insert("store".to_string(), Value::Bool(store));
    }
    body.insert("input".to_string(), Value::Array(input));

    if !instructions.is_empty() {
      body.insert(
        "instructions".to_string(),
        Value::String(instructions.join("\n\n")),
      );
    }
    if let Some(temperature) = request.temperature {
      body.insert("temperature".to_string(), serde_json::json!(temperature));
    }
    if let Some(top_p) = request.top_p {
      body.insert("top_p".to_string(), serde_json::json!(top_p));
    }
    if let Some(user) = request.user {
      body.insert("user".to_string(), Value::String(user));
    }
    if let Some(tools) = request.tools
      && !tools.is_empty()
    {
      body.insert(
        "tools".to_string(),
        Value::Array(tools.into_iter().map(tool_to_response_tool).collect()),
      );
      body.insert("parallel_tool_calls".to_string(), Value::Bool(true));
    }
    if let Some(choice) = request.tool_choice {
      body.insert("tool_choice".to_string(), Value::String(choice));
    }

    Value::Object(body)
  }
}

#[async_trait]
impl ModelProvider for OpenAICodexProvider {
  fn provider_id(&self) -> &'static str {
    "openai"
  }

  fn provider_name(&self) -> &'static str {
    "OpenAI Codex"
  }

  fn default_models(&self) -> Vec<&'static str> {
    OPENAI_CODEX_MODELS.to_vec()
  }

  async fn chat_completion(&self, request: ChatRequest) -> Result<ChatResponse> {
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    let mut stream = self.chat_completion_stream(request).await?;

    use futures::StreamExt;
    while let Some(item) = stream.next().await {
      match item? {
        Chunk::Content { delta } => content.push_str(&delta.text),
        Chunk::ToolCall { delta } => {
          if let (Some(id), Some(name), Some(arguments)) = (delta.id, delta.name, delta.arguments) {
            tool_calls.push(ToolCall {
              id,
              call_type: "function".to_string(),
              function: super::super::types::ToolCallFunction { name, arguments },
              provider_meta: None,
            });
          }
        }
        Chunk::MessageStop => break,
        _ => {}
      }
    }

    Ok(ChatResponse {
      id: "codex-response".to_string(),
      object_type: "chat.completion".to_string(),
      created: chrono::Utc::now().timestamp().max(0) as u64,
      model: "codex".to_string(),
      choices: vec![Choice {
        index: 0,
        message: ChoiceMessage {
          role: "assistant".to_string(),
          content: if content.is_empty() {
            None
          } else {
            Some(content)
          },
          tool_calls: if tool_calls.is_empty() {
            None
          } else {
            Some(tool_calls)
          },
        },
        finish_reason: Some("stop".to_string()),
      }],
      usage: Usage::default(),
      extra: Default::default(),
    })
  }

  async fn chat_completion_stream(
    &self,
    request: ChatRequest,
  ) -> Result<std::pin::Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>> {
    let events = self.responses_stream(request).await?;
    Ok(response_event_stream_to_chunk_stream(events))
  }

  async fn responses_stream(&self, request: ChatRequest) -> Result<ResponseEventStream> {
    let body = self.build_responses_body(request);
    let response = self
      .authorized_request(
        self
          .client
          .post(CODEX_API_ENDPOINT)
          .header("Content-Type", "application/json"),
      )
      .await?
      .json(&body)
      .send()
      .await
      .map_err(ModelError::NetworkError)?;

    Ok(create_openai_responses_event_stream(response))
  }

  async fn list_models(&self) -> Result<ListModelsResponse> {
    Ok(ListModelsResponse {
      object_type: "list".to_string(),
      data: OPENAI_CODEX_MODELS
        .iter()
        .map(|model| ModelInfo {
          id: (*model).to_string(),
          object_type: "model".to_string(),
          created: 0,
          owned_by: Some("openai-codex".to_string()),
        })
        .collect(),
    })
  }

  async fn validate_auth(&self) -> Result<()> {
    self.ensure_access_token().await.map(|_| ())
  }

  fn client(&self) -> &Client {
    &self.client
  }

  fn config(&self) -> &ProviderConfig {
    &self.config
  }
}

#[derive(Debug, Deserialize)]
struct RefreshTokenResponse {
  access_token: String,
  #[serde(default)]
  refresh_token: Option<String>,
  #[serde(default)]
  expires_in: Option<u64>,
  #[serde(default)]
  id_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIClaims {
  #[serde(default)]
  chatgpt_account_id: Option<String>,
  #[serde(default)]
  organization_id: Option<String>,
  #[serde(default)]
  organizations: Vec<OpenAIOrganization>,
  #[serde(default, rename = "https://api.openai.com/auth")]
  auth: Option<OpenAIAuthClaims>,
}

#[derive(Debug, Deserialize)]
struct OpenAIAuthClaims {
  #[serde(default)]
  chatgpt_account_id: Option<String>,
  #[serde(default)]
  organization_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIOrganization {
  id: String,
}

fn extract_openai_account_id_from_jwt(token: &str) -> Option<String> {
  let payload = token.split('.').nth(1)?;
  let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
    .decode(payload)
    .ok()?;
  let claims: OpenAIClaims = serde_json::from_slice(&decoded).ok()?;
  claims
    .chatgpt_account_id
    .or(claims.organization_id)
    .or_else(|| {
      claims
        .auth
        .and_then(|auth| auth.chatgpt_account_id.or(auth.organization_id))
    })
    .or_else(|| claims.organizations.first().map(|org| org.id.clone()))
}

fn tool_to_response_tool(tool: Tool) -> Value {
  let function = tool.function.unwrap_or(FunctionDefinition {
    name: String::new(),
    description: String::new(),
    parameters: serde_json::json!({}),
  });
  serde_json::json!({
    "type": "function",
    "name": function.name,
    "description": function.description,
    "parameters": function.parameters,
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::model::types::Message;

  #[test]
  fn extracts_account_id_from_organization_claims() {
    let token = format!(
      "a.{}.c",
      base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(r#"{"organizations":[{"id":"org_123"}]}"#)
    );
    assert_eq!(
      extract_openai_account_id_from_jwt(&token).as_deref(),
      Some("org_123")
    );
  }

  #[test]
  fn codex_responses_body_sets_store_false() {
    let stored = StoredCredentials::new(
      "openai-codex",
      Credentials::OAuth {
        access_token: "access".to_string(),
        refresh_token: "refresh".to_string(),
        expires_at: u64::MAX,
        account_id: Some("org_123".to_string()),
        enterprise_url: None,
      },
    );
    let mut config = ProviderConfig::default();
    config.provider_id = "openai".to_string();
    config.headers.insert(
      "x-cokra-connect-source".to_string(),
      "openai-codex".to_string(),
    );
    let provider = OpenAICodexProvider::new(&stored, config).expect("provider");
    let body = provider.build_responses_body(ChatRequest {
      model: "gpt-5.3-codex".to_string(),
      messages: vec![Message::User("hello".to_string())],
      temperature: Some(0.2),
      max_tokens: Some(2048),
      ..Default::default()
    });

    assert_eq!(body.get("store").and_then(Value::as_bool), Some(false));
    assert!(body.get("temperature").is_none());
    assert!(body.get("max_output_tokens").is_none());
  }

  #[tokio::test]
  async fn authorized_request_sets_opencode_originator_header() {
    let stored = StoredCredentials::new(
      "openai-codex",
      Credentials::OAuth {
        access_token: "access".to_string(),
        refresh_token: "refresh".to_string(),
        expires_at: u64::MAX,
        account_id: Some("org_123".to_string()),
        enterprise_url: None,
      },
    );
    let provider = OpenAICodexProvider::new(&stored, ProviderConfig::default()).expect("provider");

    let request = provider
      .authorized_request(provider.client.get("https://example.com/test"))
      .await
      .expect("authorized request")
      .build()
      .expect("built request");

    assert_eq!(
      request
        .headers()
        .get("originator")
        .and_then(|v| v.to_str().ok()),
      Some(CODEX_ORIGINATOR)
    );
    assert_eq!(
      request
        .headers()
        .get("ChatGPT-Account-Id")
        .and_then(|v| v.to_str().ok()),
      Some("org_123")
    );
  }
}
