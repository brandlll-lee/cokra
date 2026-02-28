//! Model layer types
//!
//! Core types for LLM interactions including requests, responses, and tool definitions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Chat completion request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
  /// Model identifier (e.g., "gpt-4o", "claude-sonnet-4-20250514")
  pub model: String,

  /// List of messages
  pub messages: Vec<Message>,

  /// Sampling temperature (0.0 to 2.0)
  #[serde(default)]
  pub temperature: Option<f32>,

  /// Maximum tokens to generate
  #[serde(default)]
  pub max_tokens: Option<u32>,

  /// Tools available for the model to call
  #[serde(default)]
  pub tools: Option<Vec<Tool>>,

  /// Whether to stream responses
  #[serde(default)]
  pub stream: bool,

  /// Stop sequences
  #[serde(default)]
  pub stop: Option<Vec<String>>,

  /// Presence penalty
  #[serde(default)]
  pub presence_penalty: Option<f32>,

  /// Frequency penalty
  #[serde(default)]
  pub frequency_penalty: Option<f32>,

  /// Top p (nucleus sampling)
  #[serde(default)]
  pub top_p: Option<f32>,

  /// User identifier
  #[serde(default)]
  pub user: Option<String>,
}

impl Default for ChatRequest {
  fn default() -> Self {
    Self {
      model: String::new(),
      messages: Vec::new(),
      temperature: None,
      max_tokens: None,
      tools: None,
      stream: false,
      stop: None,
      presence_penalty: None,
      frequency_penalty: None,
      top_p: None,
      user: None,
    }
  }
}

/// Message in a conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", content = "content")]
pub enum Message {
  /// System message
  System(String),

  /// User message
  User(String),

  /// Assistant message
  Assistant {
    /// Content of the message
    content: Option<String>,

    /// Tool calls made by the assistant
    #[serde(default)]
    tool_calls: Option<Vec<ToolCall>>,
  },

  /// Tool result message
  Tool {
    /// ID of the tool call this result is for
    tool_call_id: String,

    /// Content of the tool result
    content: String,
  },
}

impl Message {
  /// Create a system message
  pub fn system(content: impl Into<String>) -> Self {
    Message::System(content.into())
  }

  /// Create a user message
  pub fn user(content: impl Into<String>) -> Self {
    Message::User(content.into())
  }

  /// Create an assistant message
  pub fn assistant(content: Option<String>, tool_calls: Option<Vec<ToolCall>>) -> Self {
    Message::Assistant {
      content,
      tool_calls,
    }
  }

  /// Create a tool message
  pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
    Message::Tool {
      tool_call_id: tool_call_id.into(),
      content: content.into(),
    }
  }

  /// Get the text content of this message
  pub fn text(&self) -> Option<&str> {
    match self {
      Message::System(s) | Message::User(s) => Some(s),
      Message::Assistant { content, .. } => content.as_deref(),
      Message::Tool { content, .. } => Some(content),
    }
  }
}

/// Tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
  /// Tool type (currently only "function")
  #[serde(rename = "type")]
  pub tool_type: String,

  /// Function tool definition
  #[serde(default)]
  pub function: Option<FunctionDefinition>,
}

impl Tool {
  /// Create a function tool
  pub fn function(function: FunctionDefinition) -> Self {
    Self {
      tool_type: "function".to_string(),
      function: Some(function),
    }
  }
}

/// Function definition for a tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
  /// Name of the function
  pub name: String,

  /// Description of what the function does
  pub description: String,

  /// JSON schema for the function parameters
  #[serde(default)]
  pub parameters: serde_json::Value,
}

/// Tool call made by the model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
  /// ID of this tool call
  pub id: String,

  /// Type of tool call (currently only "function")
  #[serde(rename = "type")]
  pub call_type: String,

  /// The function to call
  pub function: ToolCallFunction,
}

/// Function call in a tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
  /// Name of the function
  pub name: String,

  /// Arguments as a JSON string
  pub arguments: String,
}

impl ToolCall {
  /// Parse the arguments as JSON
  pub fn parse_arguments<T: serde::de::DeserializeOwned>(&self) -> serde_json::Result<T> {
    serde_json::from_str(&self.function.arguments)
  }
}

/// Chat completion response
#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponse {
  /// Unique identifier for this response
  pub id: String,

  /// Type of response (e.g., "chat.completion")
  #[serde(rename = "object")]
  pub object_type: String,

  /// Unix timestamp of creation
  pub created: u64,

  /// Model used
  pub model: String,

  /// List of completion choices
  pub choices: Vec<Choice>,

  /// Usage statistics
  pub usage: Usage,

  /// Provider-specific fields
  #[serde(flatten)]
  pub extra: HashMap<String, serde_json::Value>,
}

/// A completion choice
#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
  /// Index of this choice
  pub index: u32,

  /// The message
  pub message: ChoiceMessage,

  /// Why the assistant stopped
  #[serde(default)]
  pub finish_reason: Option<String>,
}

/// Message in a choice
#[derive(Debug, Clone, Deserialize)]
pub struct ChoiceMessage {
  /// Role of the message (always "assistant")
  pub role: String,

  /// Content of the message
  #[serde(default)]
  pub content: Option<String>,

  /// Tool calls made
  #[serde(default)]
  pub tool_calls: Option<Vec<ToolCall>>,
}

/// Token usage statistics
#[derive(Debug, Clone, Deserialize, Default, Serialize)]
pub struct Usage {
  /// Number of tokens in the prompt
  #[serde(default)]
  #[serde(rename = "prompt_tokens")]
  pub input_tokens: u32,

  /// Number of tokens in the completion
  #[serde(default)]
  #[serde(rename = "completion_tokens")]
  pub output_tokens: u32,

  /// Total tokens
  #[serde(default)]
  #[serde(rename = "total_tokens")]
  pub total_tokens: u32,
}

/// Streaming chunk from the model
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum Chunk {
  /// Content chunk
  #[serde(rename = "content_block_delta")]
  Content {
    /// Delta content
    delta: ContentDelta,
  },

  /// Tool call chunk
  #[serde(rename = "tool_call_delta")]
  ToolCall {
    /// Tool call delta
    delta: ToolCallDelta,
  },

  /// Message start
  #[serde(rename = "message_start")]
  MessageStart {
    /// Message
    message: ChunkMessage,
  },

  /// Message delta
  #[serde(rename = "message_delta")]
  MessageDelta {
    /// Delta
    delta: MessageDelta,
  },

  /// Message stop
  #[serde(rename = "message_stop")]
  MessageStop,

  /// Unknown variant (for forward compatibility)
  #[serde(other)]
  Unknown,
}

/// Content delta in streaming
#[derive(Debug, Clone, Deserialize)]
pub struct ContentDelta {
  /// The text delta
  #[serde(default)]
  pub text: String,
}

/// Tool call delta in streaming
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCallDelta {
  /// ID of the tool call
  #[serde(default)]
  pub id: Option<String>,

  /// Name of the function
  #[serde(default)]
  pub name: Option<String>,

  /// Arguments delta
  #[serde(default)]
  pub arguments: Option<String>,
}

/// Chunk message
#[derive(Debug, Clone, Deserialize)]
pub struct ChunkMessage {
  /// Role
  pub role: String,

  /// Content
  #[serde(default)]
  pub content: Option<String>,
}

/// Message delta
#[derive(Debug, Clone, Deserialize)]
pub struct MessageDelta {
  /// Content
  #[serde(default)]
  pub content: Option<String>,

  /// Finish reason
  #[serde(default)]
  pub finish_reason: Option<String>,
}

/// Model information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
  /// Model ID
  pub id: String,

  /// Object type
  #[serde(rename = "object")]
  pub object_type: String,

  /// Created timestamp
  pub created: u64,

  /// Owned by
  #[serde(default)]
  pub owned_by: Option<String>,
}

/// List models response
#[derive(Debug, Clone, Deserialize)]
pub struct ListModelsResponse {
  /// Object type
  #[serde(rename = "object")]
  pub object_type: String,

  /// List of models
  pub data: Vec<ModelInfo>,
}

/// Provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
  /// Provider ID (e.g., "openai", "anthropic")
  pub provider_id: String,

  /// API key (optional if using OAuth)
  #[serde(default)]
  pub api_key: Option<String>,

  /// Base URL (for custom endpoints or local models)
  #[serde(default)]
  pub base_url: Option<String>,

  /// Organization (for OpenAI)
  #[serde(default)]
  pub organization: Option<String>,

  /// API version (for Anthropic)
  #[serde(default)]
  pub api_version: Option<String>,

  /// Timeout in seconds
  #[serde(default)]
  pub timeout: Option<u64>,

  /// Custom headers
  #[serde(default)]
  pub headers: HashMap<String, String>,

  /// Maximum retries
  #[serde(default)]
  pub max_retries: Option<u32>,
}

impl Default for ProviderConfig {
  fn default() -> Self {
    Self {
      provider_id: String::new(),
      api_key: None,
      base_url: None,
      organization: None,
      api_version: None,
      timeout: None,
      headers: HashMap::new(),
      max_retries: Some(3),
    }
  }
}
