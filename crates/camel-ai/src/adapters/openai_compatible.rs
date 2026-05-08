use std::time::Duration;

use async_trait::async_trait;
use camel_api::CamelError;
use serde::{Deserialize, Serialize};

use crate::traits::{ChatModel, EmbeddingModel};
use crate::types::{ChatRequest, ChatResponse, ChatRole, TokenUsage};

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

pub struct OpenAiCompatible {
    pub(crate) config: OpenAiCompatibleConfig,
    client: reqwest::Client,
}

impl OpenAiCompatible {
    /// M4 note: panics on reqwest client build failure. Phase 2: change to Result.
    pub fn new(config: OpenAiCompatibleConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("failed to build reqwest client");
        Self { config, client }
    }

    fn authenticated_post(&self, url: impl Into<String>) -> reqwest::RequestBuilder {
        let mut req = self.client.post(url.into());
        if let Some(key) = &self.config.api_key {
            req = req.bearer_auth(key);
        }
        req
    }
}

// ── OpenAI wire types ────────────────────────────────────────────────────────

#[derive(Serialize)]
struct OaiChatRequest<'a> {
    model: &'a str,
    messages: &'a [OaiMessage],
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<bool>,
}

#[derive(Serialize)]
struct OaiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OaiChatResponse {
    choices: Vec<OaiChoice>,
    usage: Option<OaiUsage>,
}

#[derive(Deserialize)]
struct OaiChoice {
    message: OaiMessageContent,
}

#[derive(Deserialize)]
struct OaiMessageContent {
    content: String,
}

#[derive(Deserialize)]
struct OaiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Serialize)]
struct OaiEmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct OaiEmbeddingResponse {
    data: Vec<OaiEmbeddingData>,
}

#[derive(Deserialize)]
struct OaiEmbeddingData {
    embedding: Vec<f32>,
}

// ── ChatModel ─────────────────────────────────────────────────────────────────

#[async_trait]
impl ChatModel for OpenAiCompatible {
    async fn complete(&self, req: ChatRequest) -> Result<ChatResponse, CamelError> {
        let messages: Vec<OaiMessage> = req
            .messages
            .iter()
            .map(|m| OaiMessage {
                role: match m.role {
                    ChatRole::System => "system".into(),
                    ChatRole::User => "user".into(),
                    ChatRole::Assistant => "assistant".into(),
                },
                content: m.content.clone(),
            })
            .collect();

        let body = OaiChatRequest {
            model: &self.config.model,
            messages: &messages,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            think: req.think,
        };

        let resp = self
            .authenticated_post(format!("{}/v1/chat/completions", self.config.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| CamelError::RouteError(format!("HTTP error: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(CamelError::RouteError(format!("API error {status}: {body}")));
        }
        let parsed: OaiChatResponse = resp
            .json()
            .await
            .map_err(|e| CamelError::RouteError(format!("JSON parse: {e}")))?;

        let content = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();

        Ok(ChatResponse {
            content,
            usage: parsed.usage.map(|u| TokenUsage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
            }),
        })
    }
}

// ── EmbeddingModel ───────────────────────────────────────────────────────────

#[async_trait]
impl EmbeddingModel for OpenAiCompatible {
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, CamelError> {
        let body = OaiEmbeddingRequest {
            model: &self.config.model,
            input: &texts,
        };

        let resp = self
            .authenticated_post(format!("{}/v1/embeddings", self.config.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| CamelError::RouteError(format!("HTTP error: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(CamelError::RouteError(format!("API error {status}: {body}")));
        }
        let parsed: OaiEmbeddingResponse = resp
            .json()
            .await
            .map_err(|e| CamelError::RouteError(format!("JSON parse: {e}")))?;

        Ok(parsed.data.into_iter().map(|d| d.embedding).collect())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_stores_model() {
        let cfg = OpenAiCompatibleConfig {
            base_url: "http://localhost:11434".into(),
            model: "qwen3.5:4b".into(),
            api_key: None,
        };
        let adapter = OpenAiCompatible::new(cfg);
        assert_eq!(adapter.config.model, "qwen3.5:4b");
    }

    #[test]
    fn config_base_url() {
        let cfg = OpenAiCompatibleConfig {
            base_url: "http://localhost:11434".into(),
            model: "qwen3.5:4b".into(),
            api_key: None,
        };
        let adapter = OpenAiCompatible::new(cfg);
        assert_eq!(adapter.config.base_url, "http://localhost:11434");
    }
}
