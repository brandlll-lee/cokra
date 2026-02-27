// Stub providers for compilation
// TODO: Full implementations

use async_trait::async_trait;
use crate::provider::{ModelProvider, LanguageModel, Credentials};
use crate::types::{GenerateRequest, GenerateResponse, Message, ChatOptions, ChatResponse, ModelInfo, ModelCapabilities};

macro_rules! stub_provider {
    ($name:ident, $id:literal, $display:literal) => {
        pub struct $name;

        impl Default for $name {
            fn default() -> Self { Self }
        }

        #[async_trait]
        impl ModelProvider for $name {
            fn id(&self) -> &str { $id }
            fn name(&self) -> &str { $display }

            async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
                Ok(vec![])
            }

            fn get_model(&self, model_id: &str) -> anyhow::Result<Box<dyn LanguageModel>> {
                anyhow::bail!("Provider {} not fully implemented", $id)
            }

            async fn is_authenticated(&self) -> bool { false }

            async fn authenticate(&mut self, _credentials: Credentials) -> anyhow::Result<()> {
                anyhow::bail!("Provider {} not fully implemented", $id)
            }
        }
    };
}

stub_provider!(OpenRouterProvider, "openrouter", "OpenRouter");
stub_provider!(GoogleProvider, "google", "Google AI");
stub_provider!(AzureProvider, "azure", "Azure OpenAI");
stub_provider!(GroqProvider, "groq", "Groq");
stub_provider!(MistralProvider, "mistral", "Mistral");
