// Model Provider Types
// Core type definitions for models and providers

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Model definition with full metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    /// Unique model identifier
    pub id: String,

    /// Provider ID this model belongs to
    pub provider_id: String,

    /// Human-readable name
    pub name: String,

    /// Model family (e.g., "gpt-4", "claude-3")
    pub family: Option<String>,

    /// Model capabilities
    pub capabilities: ModelCapabilities,

    /// Pricing information
    pub cost: Option<ModelCost>,

    /// Token limits
    pub limits: ModelLimits,

    /// Model status
    pub status: ModelStatus,

    /// Provider-specific options
    pub options: HashMap<String, serde_json::Value>,

    /// Additional headers for API calls
    pub headers: HashMap<String, String>,

    /// Release date
    pub release_date: Option<String>,

    /// Model variants (e.g., reasoning efforts)
    pub variants: HashMap<String, HashMap<String, serde_json::Value>>,
}

/// Model capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Supports temperature parameter
    pub temperature: bool,

    /// Supports reasoning/thinking mode
    pub reasoning: bool,

    /// Supports file attachments
    pub attachment: bool,

    /// Supports function/tool calling
    pub tool_call: bool,

    /// Input modalities
    pub input: InputModalities,

    /// Output modalities
    pub output: OutputModalities,

    /// Interleaved reasoning support
    pub interleaved: Option<InterleavedConfig>,
}

/// Input modalities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputModalities {
    pub text: bool,
    pub image: bool,
    pub audio: bool,
    pub video: bool,
    pub pdf: bool,
}

/// Output modalities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputModalities {
    pub text: bool,
    pub image: bool,
    pub audio: bool,
    pub video: bool,
}

/// Interleaved configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterleavedConfig {
    pub field: String, // "reasoning_content" or "reasoning_details"
}

/// Model pricing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCost {
    /// Cost per 1M input tokens (USD)
    pub input: f64,

    /// Cost per 1M output tokens (USD)
    pub output: f64,

    /// Cache costs
    pub cache: Option<CacheCost>,
}

/// Cache pricing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheCost {
    pub read: f64,
    pub write: f64,
}

/// Token limits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelLimits {
    /// Maximum context window
    pub context: usize,

    /// Maximum input tokens
    pub input: Option<usize>,

    /// Maximum output tokens
    pub output: usize,
}

/// Model status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelStatus {
    Alpha,
    Beta,
    Deprecated,
    Active,
}

/// Simplified model info for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model ID
    pub id: String,

    /// Provider ID
    pub provider_id: String,

    /// Display name
    pub name: String,

    /// Capabilities summary
    pub capabilities: ModelCapabilities,
}

impl From<Model> for ModelInfo {
    fn from(model: Model) -> Self {
        Self {
            id: model.id,
            provider_id: model.provider_id,
            name: model.name,
            capabilities: model.capabilities,
        }
    }
}

/// Provider information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// Provider ID
    pub id: String,

    /// Provider name
    pub name: String,

    /// Source of provider config
    pub source: ProviderSource,

    /// Environment variable names for API keys
    pub env: Vec<String>,

    /// Current API key (if any)
    pub key: Option<String>,

    /// Provider options
    pub options: HashMap<String, serde_json::Value>,

    /// Available models
    pub models: HashMap<String, Model>,
}

/// Provider config source
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProviderSource {
    Env,
    Config,
    Custom,
    Api,
}

/// Usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

impl Usage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Calculate blended total (non-cached input + output)
    pub fn blended_total(&self) -> i64 {
        self.input_tokens - self.cached_input_tokens + self.output_tokens
    }
}

/// Generate request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateRequest {
    /// Messages for the conversation
    pub messages: Vec<Message>,

    /// Model-specific options
    pub options: GenerateOptions,
}

/// Chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: Vec<ContentPart>,
}

/// Message role
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Content part
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentPart {
    Text { text: String },
    Image { image_url: String },
}

/// Generate options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateOptions {
    pub temperature: Option<f32>,
    pub max_tokens: Option<usize>,
    pub stop: Option<Vec<String>>,
    pub tools: Option<Vec<ToolDefinition>>,
    pub tool_choice: Option<ToolChoice>,
}

/// Tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub parameters: serde_json::Value,
}

/// Tool choice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    Function { name: String },
}

/// Generate response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: FinishReason,
    pub usage: Usage,
}

/// Tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Finish reason
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
    Error,
}
