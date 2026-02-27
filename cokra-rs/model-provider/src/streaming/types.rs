// Streaming Types and Transform
// Unified streaming interface

use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

/// Stream part types - unified across all providers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamPart {
    /// Stream start with warnings
    StreamStart {
        warnings: Vec<StreamWarning>,
    },

    /// Response metadata
    ResponseMetadata {
        id: Option<String>,
        model: Option<String>,
    },

    /// Reasoning content start
    ReasoningStart {
        id: String,
    },

    /// Reasoning content delta
    ReasoningDelta {
        id: String,
        delta: String,
    },

    /// Reasoning content end
    ReasoningEnd {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },

    /// Text content start
    TextStart {
        id: String,
    },

    /// Text content delta
    TextDelta {
        id: String,
        delta: String,
    },

    /// Text content end
    TextEnd {
        id: String,
    },

    /// Tool input start
    ToolInputStart {
        id: String,
        tool_name: String,
    },

    /// Tool input delta
    ToolInputDelta {
        id: String,
        delta: String,
    },

    /// Tool input end
    ToolInputEnd {
        id: String,
    },

    /// Complete tool call
    ToolCall {
        tool_call_id: String,
        tool_name: String,
        input: String,
    },

    /// Stream finish
    Finish {
        finish_reason: String,
        usage: StreamUsage,
        provider_metadata: Option<ProviderMetadata>,
    },

    /// Error
    Error {
        error: String,
    },
}

/// Stream warning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamWarning {
    pub kind: String,
    pub message: String,
}

/// Provider metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderMetadata {
    pub provider: String,
    pub data: serde_json::Value,
}

/// Stream usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

impl StreamUsage {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Transform state for streaming
#[derive(Debug, Default)]
pub struct TransformState {
    pub is_active_reasoning: bool,
    pub is_active_text: bool,
    pub current_text_id: Option<String>,
    pub current_reasoning_id: Option<String>,
    pub tool_calls_in_progress: Vec<ToolCallState>,
}

/// Tool call state during streaming
#[derive(Debug, Clone)]
pub struct ToolCallState {
    pub id: String,
    pub name: String,
    pub arguments: String,
    pub is_complete: bool,
}

/// Stream transform configuration
#[derive(Debug, Clone)]
pub struct StreamTransformConfig {
    /// Include usage in finish
    pub include_usage: bool,

    /// Provider name for metadata
    pub provider_name: String,

    /// Supported features
    pub supports_reasoning: bool,
    pub supports_tools: bool,
}

impl Default for StreamTransformConfig {
    fn default() -> Self {
        Self {
            include_usage: true,
            provider_name: "unknown".to_string(),
            supports_reasoning: false,
            supports_tools: true,
        }
    }
}
