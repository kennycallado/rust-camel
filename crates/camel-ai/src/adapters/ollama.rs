use async_trait::async_trait;
use camel_api::CamelError;
use ollama_rs::{
    generation::{
        chat::request::ChatMessageRequest,
        embeddings::request::{EmbeddingsInput, GenerateEmbeddingsRequest},
        parameters::ThinkType,
    },
    models::ModelOptions,
    Ollama,
};

use crate::traits::{ChatModel, EmbeddingModel};
use crate::types::{ChatRequest, ChatResponse, ChatRole};

// ---------------------------------------------------------------------------
// Config & Adapter structs
// ---------------------------------------------------------------------------

/// Configuration for connecting to an Ollama instance.
#[derive(Debug, Clone)]
pub struct OllamaConfig {
    /// Base URL of the Ollama service, e.g. `"http://localhost:11434"`.
    pub base_url: String,
    /// Model identifier, e.g. `"qwen3.5:4b"`.
    pub model: String,
}

/// Adapter that talks to Ollama via the `ollama-rs` crate.
#[derive(Debug, Clone)]
pub struct OllamaAdapter {
    pub(crate) config: OllamaConfig,
    client: Ollama,
}

impl OllamaAdapter {
    pub fn new(config: OllamaConfig) -> Self {
        let client = build_client(&config.base_url);
        Self { config, client }
    }
}

/// Build an `Ollama` client by parsing `base_url` into host + port.
///
/// `Ollama::new` expects the host WITHOUT a port suffix and the port as a
/// separate `u16`. We do this manually to avoid adding a `url` crate
/// dependency to `camel-ai`.
fn build_client(base_url: &str) -> Ollama {
    let base = base_url.trim_end_matches('/');
    let without_scheme = base
        .strip_prefix("http://")
        .or_else(|| base.strip_prefix("https://"))
        .unwrap_or(base);
    let (host_part, port) = if let Some((h, p)) = without_scheme.rsplit_once(':') {
        let port = p.parse::<u16>().unwrap_or(11434);
        (h.to_string(), port)
    } else {
        (without_scheme.to_string(), 11434u16)
    };
    let scheme = if base.starts_with("https://") {
        "https"
    } else {
        "http"
    };
    Ollama::new(format!("{scheme}://{host_part}"), port)
}

// ---------------------------------------------------------------------------
// ChatModel implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ChatModel for OllamaAdapter {
    async fn complete(&self, req: ChatRequest) -> Result<ChatResponse, CamelError> {
        // Map camel-ai messages → ollama-rs ChatMessage
        let messages: Vec<ollama_rs::generation::chat::ChatMessage> = req
            .messages
            .iter()
            .map(|m| match m.role {
                ChatRole::System => {
                    ollama_rs::generation::chat::ChatMessage::system(m.content.clone())
                }
                ChatRole::User => {
                    ollama_rs::generation::chat::ChatMessage::user(m.content.clone())
                }
                ChatRole::Assistant => {
                    ollama_rs::generation::chat::ChatMessage::assistant(m.content.clone())
                }
            })
            .collect();

        let mut chat_req =
            ChatMessageRequest::new(self.config.model.clone(), messages);

        // Apply think flag
        if let Some(think) = req.think {
            chat_req = chat_req.think(ThinkType::from(think));
        }

        // Apply generation options (temperature, num_predict) if present
        if req.temperature.is_some() || req.max_tokens.is_some() {
            let mut opts = ModelOptions::default();
            if let Some(temp) = req.temperature {
                opts = opts.temperature(temp);
            }
            if let Some(max_tokens) = req.max_tokens {
                opts = opts.num_predict(max_tokens as i32);
            }
            chat_req = chat_req.options(opts);
        }

        let response = self.client
            .send_chat_messages(chat_req)
            .await
            .map_err(|e| CamelError::RouteError(e.to_string()))?;

        let content = response.message.content;

        Ok(ChatResponse {
            content,
            usage: None, // ollama-rs doesn't return structured token counts in this API
        })
    }
}

// ---------------------------------------------------------------------------
// EmbeddingModel implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl EmbeddingModel for OllamaAdapter {
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, CamelError> {
        if texts.len() == 1 {
            // Single-text fast path — avoids allocating a vec for one element
            let req = GenerateEmbeddingsRequest::new(
                self.config.model.clone(),
                EmbeddingsInput::Single(texts.into_iter().next().unwrap()),
            );
            let resp = self.client
                .generate_embeddings(req)
                .await
                .map_err(|e| CamelError::RouteError(e.to_string()))?;
            return Ok(resp.embeddings);
        }

        // Batch: pass all texts at once using EmbeddingsInput::Multiple
        let req = GenerateEmbeddingsRequest::new(
            self.config.model.clone(),
            EmbeddingsInput::Multiple(texts),
        );
        let resp = self.client
            .generate_embeddings(req)
            .await
            .map_err(|e| CamelError::RouteError(e.to_string()))?;
        Ok(resp.embeddings)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_stores_model() {
        let adapter = OllamaAdapter::new(OllamaConfig {
            base_url: "http://localhost:11434".into(),
            model: "qwen3.5:4b".into(),
        });
        assert_eq!(adapter.config.model, "qwen3.5:4b");
    }

    #[test]
    fn config_stores_base_url() {
        let adapter = OllamaAdapter::new(OllamaConfig {
            base_url: "http://myhost:11434".into(),
            model: "llama3".into(),
        });
        assert_eq!(adapter.config.base_url, "http://myhost:11434");
    }

    #[test]
    fn client_is_stored() {
        let adapter = OllamaAdapter::new(OllamaConfig {
            base_url: "http://myhost:9999".into(),
            model: "test".into(),
        });
        // verify construction succeeds (client stored internally)
        assert_eq!(adapter.config.base_url, "http://myhost:9999");
    }
}
