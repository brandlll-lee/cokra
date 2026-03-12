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

pub mod auth_orchestrator;
pub mod client;
pub mod error;
pub mod metadata;
pub mod models_dev;
pub mod oauth_connect;
pub mod plugin_registry;
pub mod provider;
pub mod provider_catalog;
pub mod registry;
pub mod streaming;
pub mod transform;
pub mod types;

pub mod auth;
pub mod auth_store;
pub mod providers;

// Re-exports
pub use auth_orchestrator::ProviderAuth;
pub use client::ModelClient;
pub use error::ModelError;
pub use error::Result;
pub use metadata::ModelMetadata;
pub use metadata::ModelMetadataManager;
pub use plugin_registry::PluginRegistry;
pub use provider::ModelProvider;
pub use provider::ProviderInfo;
pub use provider_catalog::ProviderCatalogEntry;
pub use provider_catalog::RuntimeRegistrationKind;
pub use registry::ProviderRegistry;
pub use streaming::AnthropicUsageParser;
pub use streaming::OpenAIUsageParser;
pub use streaming::StreamingConfig;
pub use streaming::StreamingProcessor;
pub use streaming::UsageParser;
pub use transform::AnthropicTransform;
pub use transform::EmptyContentHandling;
pub use transform::MessageTransform;
pub use transform::OpenAICompatibleTransform;
pub use transform::ProviderRuntimeKind;
pub use transform::ProviderRuntimeTransform;
pub use transform::RuntimeRequestDefaults;
pub use transform::StreamChunk;
pub use transform::ToolCallIdFormat;
pub use transform::TransformConfig;
pub use transform::normalize_tool_call_id_for_mistral;
pub use transform::transform_for_provider;
pub use types::*;

use std::sync::Arc;

/// Default providers to register
pub async fn register_default_providers(_registry: &ProviderRegistry) -> Result<()> {
  // Providers will be registered with their specific implementations
  // This is called during Cokra initialization
  Ok(())
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
