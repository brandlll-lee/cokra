//! Cokra Model Provider Layer
//!
//! This module provides a unified abstraction for multiple LLM providers,
//! supporting OpenAI, Anthropic, Ollama, LM Studio, GitHub, and more.
//!
//! Architecture:
//! - [ModelProvider] trait: defines the interface for all providers
//! - [ProviderRegistry]: manages provider registration and discovery
//! - [AuthManager]: handles authentication (API Key, OAuth, Bearer Token)
//! - Provider implementations in [providers]

pub mod client;
pub mod error;
pub mod metadata;
pub mod provider;
pub mod registry;
pub mod streaming;
pub mod transform;
pub mod types;

pub mod auth;
pub mod providers;

// Re-exports
pub use client::ModelClient;
pub use error::{ModelError, Result};
pub use metadata::{ModelMetadata, ModelMetadataManager};
pub use provider::{ModelProvider, ProviderInfo};
pub use registry::ProviderRegistry;
pub use streaming::{
  AnthropicUsageParser, OpenAIUsageParser, StreamingConfig, StreamingProcessor, UsageParser,
};
pub use transform::{
  AnthropicTransform, EmptyContentHandling, MessageTransform, OpenAICompatibleTransform,
  StreamChunk, ToolCallIdFormat, TransformConfig, normalize_tool_call_id_for_mistral,
  transform_for_provider,
};
pub use types::*;

use std::sync::Arc;

/// Default providers to register
pub fn register_default_providers(
  registry: &ProviderRegistry,
) -> impl std::future::Future<Output = Result<()>> + '_ {
  async move {
    // Providers will be registered with their specific implementations
    // This is called during Cokra initialization
    Ok(())
  }
}

/// Initialize the model layer with default configuration
pub async fn init_model_layer(config: &cokra_config::Config) -> Result<Arc<ModelClient>> {
  let registry = Arc::new(ProviderRegistry::new());

  // Register all providers
  providers::register_all_providers(&registry, config).await?;

  // Create model client with default provider
  let client = ModelClient::new(registry).await?;

  Ok(Arc::new(client))
}
