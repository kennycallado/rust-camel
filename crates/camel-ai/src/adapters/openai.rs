use async_trait::async_trait;
use camel_api::CamelError;

use crate::traits::{ChatModel, EmbeddingModel};
use crate::types::{ChatRequest, ChatResponse, ChatRole, TokenUsage};

#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    /// None = OpenAI default base URL. Some = custom (for OpenAI-compat providers).
    pub base_url: Option<String>,
    pub model: String,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OpenAiAdapter {
    pub(crate) config: OpenAiConfig,
}

impl OpenAiAdapter {
    pub fn new(config: OpenAiConfig) -> Self {
        Self { config }
    }

    fn client(&self) -> async_openai::Client<async_openai::config::OpenAIConfig> {
        let mut cfg = async_openai::config::OpenAIConfig::default();
        if let Some(ref key) = self.config.api_key {
            cfg = cfg.with_api_key(key);
        }
        if let Some(ref base) = self.config.base_url {
            // async-openai appends paths directly to api_base (no /v1 added automatically).
            // Ensure the base ends with /v1 so endpoints like /v1/embeddings resolve correctly.
            let normalized = if base.ends_with("/v1") {
                base.clone()
            } else {
                format!("{}/v1", base.trim_end_matches('/'))
            };
            cfg = cfg.with_api_base(normalized);
        }
        async_openai::Client::with_config(cfg)
    }
}

#[async_trait]
impl ChatModel for OpenAiAdapter {
    async fn complete(&self, req: ChatRequest) -> Result<ChatResponse, CamelError> {
        let messages = req
            .messages
            .into_iter()
            .map(|m| {
                match m.role {
                    ChatRole::System => {
                        let built = async_openai::types::chat::ChatCompletionRequestSystemMessageArgs::default()
                            .content(m.content)
                            .build()
                            .map_err(|e| {
                                CamelError::RouteError(format!("OpenAI message build error: {e}"))
                            })?;
                        Ok(async_openai::types::chat::ChatCompletionRequestMessage::System(built))
                    }
                    ChatRole::User => {
                        let built = async_openai::types::chat::ChatCompletionRequestUserMessageArgs::default()
                            .content(m.content)
                            .build()
                            .map_err(|e| {
                                CamelError::RouteError(format!("OpenAI message build error: {e}"))
                            })?;
                        Ok(async_openai::types::chat::ChatCompletionRequestMessage::User(built))
                    }
                    ChatRole::Assistant => {
                        let built = async_openai::types::chat::ChatCompletionRequestAssistantMessageArgs::default()
                            .content(m.content)
                            .build()
                            .map_err(|e| {
                                CamelError::RouteError(format!("OpenAI message build error: {e}"))
                            })?;
                        Ok(async_openai::types::chat::ChatCompletionRequestMessage::Assistant(
                            built,
                        ))
                    }
                }
            })
            .collect::<Result<Vec<_>, CamelError>>()?;

        let mut request = async_openai::types::chat::CreateChatCompletionRequestArgs::default();
        request.model(&self.config.model).messages(messages);
        if let Some(temp) = req.temperature {
            request.temperature(temp);
        }
        if let Some(max_tokens) = req.max_tokens {
            request.max_tokens(max_tokens);
        }
        let request = request
            .build()
            .map_err(|e| CamelError::RouteError(format!("OpenAI request build error: {e}")))?;

        let response = self
            .client()
            .chat()
            .create(request)
            .await
            .map_err(|e| CamelError::RouteError(format!("OpenAI chat error: {e}")))?;

        Ok(ChatResponse {
            content: response
                .choices
                .first()
                .and_then(|c| c.message.content.clone())
                .unwrap_or_default(),
            usage: response.usage.map(|u| TokenUsage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
            }),
        })
    }
}

#[async_trait]
impl EmbeddingModel for OpenAiAdapter {
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, CamelError> {
        let request = async_openai::types::embeddings::CreateEmbeddingRequestArgs::default()
            .model(&self.config.model)
            .input(async_openai::types::embeddings::EmbeddingInput::StringArray(texts))
            .build()
            .map_err(|e| {
                CamelError::RouteError(format!("OpenAI embedding request build error: {e}"))
            })?;

        let mut response = self
            .client()
            .embeddings()
            .create(request)
            .await
            .map_err(|e| CamelError::RouteError(format!("OpenAI embedding error: {e}")))?;

        response.data.sort_by_key(|d| d.index);
        for (i, d) in response.data.iter().enumerate() {
            if d.index as usize != i {
                return Err(CamelError::RouteError(format!(
                    "embedding response has non-contiguous indices at position {i}: got {}",
                    d.index
                )));
            }
        }
        Ok(response.data.into_iter().map(|d| d.embedding).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_stores_model() {
        let adapter = OpenAiAdapter::new(OpenAiConfig {
            base_url: None,
            model: "gpt-4o".into(),
            api_key: Some("sk-test".into()),
        });
        assert_eq!(adapter.config.model, "gpt-4o");
    }

    #[test]
    fn config_custom_base_url() {
        let adapter = OpenAiAdapter::new(OpenAiConfig {
            base_url: Some("http://localhost:11434/v1".into()),
            model: "qwen3.5:4b".into(),
            api_key: None,
        });
        assert_eq!(
            adapter.config.base_url.as_deref(),
            Some("http://localhost:11434/v1")
        );
    }
}
