// Model Registry
// Central registry for all models and providers

use std::collections::HashMap;
use std::sync::Arc;

use crate::provider::{ModelProvider, LanguageModel, ProviderError, ModelNotFoundError};
use crate::types::{Model, ModelInfo, ProviderInfo};

/// Model registry - stores all providers and models
pub struct ModelRegistry {
    /// Providers by ID
    providers: HashMap<String, Arc<dyn ModelProvider>>,

    /// Cached model info
    models: HashMap<String, ModelInfo>,

    /// Default provider
    default_provider: String,
}

impl ModelRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            models: HashMap::new(),
            default_provider: "openai".to_string(),
        }
    }

    /// Register a provider
    pub fn register(&mut self, provider: Arc<dyn ModelProvider>) {
        let id = provider.id().to_string();
        self.providers.insert(id, provider);
    }

    /// Set default provider
    pub fn set_default_provider(&mut self, provider_id: &str) -> anyhow::Result<()> {
        if !self.providers.contains_key(provider_id) {
            anyhow::bail!("Provider not found: {}", provider_id);
        }
        self.default_provider = provider_id.to_string();
        Ok(())
    }

    /// Get a provider by ID
    pub fn get_provider(&self, provider_id: &str) -> Option<Arc<dyn ModelProvider>> {
        self.providers.get(provider_id).cloned()
    }

    /// Get a model by provider and model ID
    pub async fn get_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Box<dyn LanguageModel>, ProviderError> {
        let provider = self.providers.get(provider_id)
            .ok_or_else(|| ProviderError::ProviderNotFound(provider_id.to_string()))?;

        provider.get_model(model_id).map_err(|_| {
            ProviderError::ModelNotFound {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
            }
        })
    }

    /// Get model by full string "provider/model"
    pub async fn get_model_by_string(
        &self,
        model_str: &str,
    ) -> Result<Box<dyn LanguageModel>, ProviderError> {
        let (provider_id, model_id) = self.parse_model_string(model_str);
        self.get_model(&provider_id, &model_id).await
    }

    /// Parse model string "provider/model" or "model"
    pub fn parse_model_string(&self, model_str: &str) -> (String, String) {
        if let Some((provider, model)) = model_str.split_once('/') {
            (provider.to_string(), model.to_string())
        } else {
            (self.default_provider.clone(), model_str.to_string())
        }
    }

    /// List all providers
    pub fn list_providers(&self) -> Vec<&str> {
        self.providers.keys().map(|s| s.as_str()).collect()
    }

    /// List models for a provider
    pub async fn list_models(&self, provider_id: Option<&str>) -> anyhow::Result<Vec<ModelInfo>> {
        if let Some(id) = provider_id {
            let provider = self.providers.get(id)
                .ok_or_else(|| anyhow::anyhow!("Provider not found: {}", id))?;
            provider.list_models().await
        } else {
            let mut all_models = Vec::new();
            for provider in self.providers.values() {
                if let Ok(models) = provider.list_models().await {
                    all_models.extend(models);
                }
            }
            Ok(all_models)
        }
    }

    /// Refresh model cache
    pub async fn refresh(&mut self) -> anyhow::Result<()> {
        self.models.clear();

        for (provider_id, provider) in &self.providers {
            if let Ok(models) = provider.list_models().await {
                for model in models {
                    let key = format!("{}/{}", provider_id, model.id);
                    self.models.insert(key, model);
                }
            }
        }

        Ok(())
    }
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}
