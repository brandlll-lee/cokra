// Model Provider Trait
// Core trait definitions for model providers

use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

use crate::types::{GenerateRequest, GenerateResponse, Message, ModelInfo, ToolCall, Usage};

/// Model provider trait - implemented by each provider
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Get provider ID
    fn id(&self) -> &str;

    /// Get provider name
    fn name(&self) -> &str;

    /// List available models
    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>>;

    /// Get a specific model
    fn get_model(&self, model_id: &str) -> anyhow::Result<Box<dyn LanguageModel>>;

    /// Check if authenticated
    async fn is_authenticated(&self) -> bool;

    /// Authenticate with credentials
    async fn authenticate(&mut self, credentials: Credentials) -> anyhow::Result<()>;
}

/// Language model trait - core model interface
#[async_trait]
pub trait LanguageModel: Send + Sync {
    /// Get model ID
    fn id(&self) -> &str;

    /// Get model capabilities
    fn capabilities(&self) -> &crate::types::ModelCapabilities;

    /// Generate text (non-streaming)
    async fn generate(&self, request: GenerateRequest) -> anyhow::Result<GenerateResponse>;

    /// Generate text (streaming)
    async fn generate_stream(
        &self,
        request: GenerateRequest,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamChunk>> + Send>>>;

    /// Chat completion (non-streaming)
    async fn chat(&self, messages: Vec<Message>, options: ChatOptions) -> anyhow::Result<ChatResponse>;

    /// Chat completion (streaming)
    async fn chat_stream(
        &self,
        messages: Vec<Message>,
        options: ChatOptions,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<ChatChunk>> + Send>>>;
}

/// Chat model trait - extended chat capabilities
#[async_trait]
pub trait ChatModel: LanguageModel {
    /// Whether model supports structured outputs
    fn supports_structured_outputs(&self) -> bool;

    /// Whether model supports tool calling
    fn supports_tools(&self) -> bool;

    /// Whether model supports vision
    fn supports_vision(&self) -> bool;
}

/// Stream chunk
#[derive(Debug, Clone)]
pub struct StreamChunk {
    pub delta: String,
    pub tool_calls: Vec<ToolCallDelta>,
    pub finish_reason: Option<crate::types::FinishReason>,
    pub usage: Option<Usage>,
}

/// Tool call delta (streaming)
#[derive(Debug, Clone)]
pub struct ToolCallDelta {
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments_delta: String,
}

/// Chat options
#[derive(Debug, Clone, Default)]
pub struct ChatOptions {
    pub temperature: Option<f32>,
    pub max_tokens: Option<usize>,
    pub tools: Option<Vec<crate::types::ToolDefinition>>,
    pub tool_choice: Option<crate::types::ToolChoice>,
    pub system_prompt: Option<String>,
}

/// Chat response
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub message: Message,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: crate::types::FinishReason,
    pub usage: Usage,
}

/// Chat chunk (streaming)
#[derive(Debug, Clone)]
pub struct ChatChunk {
    pub delta: Option<String>,
    pub tool_call_delta: Option<ToolCallDelta>,
    pub finish_reason: Option<crate::types::FinishReason>,
    pub usage: Option<Usage>,
}

/// Credentials for authentication
#[derive(Debug, Clone)]
pub enum Credentials {
    /// API key authentication
    ApiKey { key: String },

    /// OAuth authentication
    OAuth {
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<i64>,
    },

    /// No credentials needed
    None,
}

/// Provider error
#[derive(thiserror::Error, Debug)]
pub enum ProviderError {
    #[error("Model not found: {provider_id}/{model_id}")]
    ModelNotFound {
        provider_id: String,
        model_id: String,
    },

    #[error("Provider not found: {0}")]
    ProviderNotFound(String),

    #[error("Authentication required for {0}")]
    AuthenticationRequired(String),

    #[error("API call failed: {message}")]
    ApiCall {
        message: String,
        status_code: Option<u16>,
        is_retryable: bool,
    },

    #[error("Stream error: {0}")]
    StreamError(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),
}

/// Model not found error
#[derive(Debug)]
pub struct ModelNotFoundError {
    pub provider_id: String,
    pub model_id: String,
    pub suggestions: Vec<String>,
}

impl std::fmt::Display for ModelNotFoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Model not found: {}/{}",
            self.provider_id, self.model_id
        )
    }
}

impl std::error::Error for ModelNotFoundError {}
