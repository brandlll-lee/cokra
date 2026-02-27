// Custom Provider
// For user-defined providers

use async_trait::async_trait;

use crate::provider::{ModelProvider, LanguageModel, Credentials};
use crate::types::{GenerateRequest, GenerateResponse, Message, ChatOptions, ChatResponse, ModelInfo, ModelCapabilities};

/// Custom provider configuration
pub struct CustomProvider {
    id: String,
    name: String,
    base_url: String,
    api_key: Option<String>,
    models: Vec<ModelInfo>,
}

impl CustomProvider {
    pub fn new(id: &str, name: &str, base_url: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            base_url: base_url.to_string(),
            api_key: None,
            models: vec![],
        }
    }

    pub fn with_api_key(mut self, key: &str) -> Self {
        self.api_key = Some(key.to_string());
        self
    }

    pub fn with_model(mut self, model: ModelInfo) -> Self {
        self.models.push(model);
        self
    }
}

#[async_trait]
impl ModelProvider for CustomProvider {
    fn id(&self) -> &str { &self.id }
    fn name(&self) -> &str { &self.name }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(self.models.clone())
    }

    fn get_model(&self, model_id: &str) -> anyhow::Result<Box<dyn LanguageModel>> {
        let model_info = self.models.iter()
            .find(|m| m.id == model_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

        Ok(Box::new(CustomModel {
            model_id: model_id.to_string(),
            model_info,
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
        }))
    }

    async fn is_authenticated(&self) -> bool {
        self.api_key.is_some()
    }

    async fn authenticate(&mut self, credentials: Credentials) -> anyhow::Result<()> {
        match credentials {
            Credentials::ApiKey { key } => {
                self.api_key = Some(key);
                Ok(())
            }
            _ => anyhow::bail!("Custom provider only supports API key authentication"),
        }
    }
}

/// Custom model
pub struct CustomModel {
    model_id: String,
    model_info: ModelInfo,
    base_url: String,
    api_key: Option<String>,
}

#[async_trait]
impl LanguageModel for CustomModel {
    fn id(&self) -> &str { &self.model_id }
    fn capabilities(&self) -> &ModelCapabilities { &self.model_info.capabilities }

    async fn generate(&self, _request: GenerateRequest) -> anyhow::Result<GenerateResponse> {
        Ok(GenerateResponse {
            content: format!("Custom provider {} response", self.model_id),
            tool_calls: vec![],
            finish_reason: crate::types::FinishReason::Stop,
            usage: crate::types::Usage::new(),
        })
    }

    async fn generate_stream(
        &self,
        _request: GenerateRequest,
    ) -> anyhow::Result<std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<crate::streaming::ProviderChunk>> + Send>>> {
        use futures::stream;
        Ok(Box::pin(stream::empty()))
    }

    async fn chat(&self, _messages: Vec<Message>, _options: ChatOptions) -> anyhow::Result<ChatResponse> {
        Ok(ChatResponse {
            message: Message {
                role: crate::types::MessageRole::Assistant,
                content: vec![crate::types::ContentPart::Text {
                    text: format!("Custom provider {} response", self.model_id),
                }],
            },
            tool_calls: vec![],
            finish_reason: crate::types::FinishReason::Stop,
            usage: crate::types::Usage::new(),
        })
    }

    async fn chat_stream(
        &self,
        _messages: Vec<Message>,
        _options: ChatOptions,
    ) -> anyhow::Result<std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<crate::provider::ChatChunk>> + Send>>> {
        use futures::stream;
        Ok(Box::pin(stream::empty()))
    }
}
