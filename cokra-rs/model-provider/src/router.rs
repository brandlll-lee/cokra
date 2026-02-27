// Model Router
// Routes model requests to appropriate providers

use std::sync::Arc;

use crate::provider::{LanguageModel, ProviderError};
use crate::registry::ModelRegistry;
use crate::types::{ChatOptions, ChatResponse, GenerateRequest, GenerateResponse, Message, Usage};
use crate::streaming::StreamChunk;

/// Model router - routes to appropriate provider
pub struct ModelRouter {
    registry: Arc<ModelRegistry>,
    default_model: String,
}

impl ModelRouter {
    /// Create a new router with registry
    pub fn new(registry: ModelRegistry) -> Self {
        Self {
            registry: Arc::new(registry),
            default_model: "openai/gpt-4o".to_string(),
        }
    }

    /// Set default model
    pub fn set_default_model(&mut self, model: &str) {
        self.default_model = model.to_string();
    }

    /// Get model by string
    pub async fn get_model(&self, model_str: Option<&str>) -> Result<Box<dyn LanguageModel>, ProviderError> {
        let model_str = model_str.unwrap_or(&self.default_model);
        self.registry.get_model_by_string(model_str).await
    }

    /// Generate with model
    pub async fn generate(
        &self,
        request: GenerateRequest,
        model: Option<&str>,
    ) -> anyhow::Result<GenerateResponse> {
        let model = self.get_model(model).await?;
        model.generate(request).await
    }

    /// Chat with model
    pub async fn chat(
        &self,
        messages: Vec<Message>,
        options: ChatOptions,
        model: Option<&str>,
    ) -> anyhow::Result<ChatResponse> {
        let model_impl = self.get_model(model).await?;
        model_impl.chat(messages, options).await
    }

    /// List available models
    pub async fn list_models(&self, provider: Option<&str>) -> anyhow::Result<Vec<crate::types::ModelInfo>> {
        self.registry.list_models(provider).await
    }

    /// List providers
    pub fn list_providers(&self) -> Vec<&str> {
        self.registry.list_providers()
    }

    /// Get registry
    pub fn registry(&self) -> Arc<ModelRegistry> {
        self.registry.clone()
    }
}

impl Default for ModelRouter {
    fn default() -> Self {
        Self::new(ModelRegistry::new())
    }
}

/// Builder for model router
pub struct ModelRouterBuilder {
    registry: ModelRegistry,
    default_model: String,
}

impl ModelRouterBuilder {
    /// Create new builder
    pub fn new() -> Self {
        Self {
            registry: ModelRegistry::new(),
            default_model: "openai/gpt-4o".to_string(),
        }
    }

    /// Add provider
    pub fn with_provider(mut self, provider: Arc<dyn crate::provider::ModelProvider>) -> Self {
        self.registry.register(provider);
        self
    }

    /// Set default model
    pub fn default_model(mut self, model: &str) -> Self {
        self.default_model = model.to_string();
        self
    }

    /// Build router
    pub fn build(self) -> ModelRouter {
        let mut router = ModelRouter::new(self.registry);
        router.set_default_model(&self.default_model);
        router
    }
}

impl Default for ModelRouterBuilder {
    fn default() -> Self {
        Self::new()
    }
}
