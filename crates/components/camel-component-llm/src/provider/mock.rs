use async_trait::async_trait;
use futures::stream::{self, BoxStream};

use crate::error::LlmError;
use crate::provider::{
    ChatEvent, ChatRequest, ChatRole, EmbedRequest, EmbedResponse, FinishReason, LlmProvider,
    LlmUsage,
};

/// Mock provider mode.
#[derive(Debug, Clone)]
pub enum MockMode {
    /// Echo user messages back verbatim.
    Echo,
    /// Return a fixed response.
    Fixed(String),
    /// Stream a partial delta then error — for mid-stream failure testing.
    Error(LlmError),
}

/// A mock LLM provider for testing.
///
/// # Modes
/// - `Echo`: streams back the concatenation of all user messages.
/// - `Fixed(String)`: streams back the given canned text.
/// - `Error(LlmError)`: sends one Delta event, then the error.
pub struct MockProvider {
    id: String,
    mode: MockMode,
    default_model: String,
}

impl MockProvider {
    /// Create a mock provider with the given id and mode.
    ///
    /// The default model is `"mock-model"`.
    pub fn new(id: impl Into<String>, mode: MockMode) -> Self {
        Self {
            id: id.into(),
            mode,
            default_model: "mock-model".to_string(),
        }
    }

    /// Set the default model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = model.into();
        self
    }

    /// Shortcut for `MockProvider::new("mock", MockMode::Echo)`.
    pub fn echo() -> Self {
        Self::new("mock", MockMode::Echo)
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn chat_stream(&self, req: ChatRequest) -> BoxStream<'static, Result<ChatEvent, LlmError>> {
        let text = match &self.mode {
            MockMode::Echo => req
                .messages
                .iter()
                .filter(|m| matches!(m.role, ChatRole::User))
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            MockMode::Fixed(t) => t.clone(),
            MockMode::Error(e) => {
                let stream = stream::iter(vec![
                    Ok(ChatEvent::Delta {
                        text: "partial".into(),
                    }),
                    Err(e.clone()),
                ]);
                return Box::pin(stream);
            }
        };

        let prompt_tokens = text.len() as u32 / 4;
        let completion_tokens = text.len() as u32 / 4;

        let stream = stream::iter(vec![
            Ok(ChatEvent::Delta { text: text.clone() }),
            Ok(ChatEvent::Finished {
                usage: Some(LlmUsage {
                    prompt_tokens,
                    completion_tokens,
                    total_tokens: prompt_tokens + completion_tokens,
                }),
                model: Some(req.model.clone()),
                finish_reason: Some(FinishReason::Stop),
                metadata: serde_json::Map::new(),
            }),
        ]);
        Box::pin(stream)
    }

    async fn embed(&self, req: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        let embeddings: Vec<Vec<f32>> = req
            .inputs
            .iter()
            .map(|input| {
                let hash = input
                    .bytes()
                    .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
                (0..16).map(|i| ((hash >> i) & 1) as f32).collect()
            })
            .collect();

        let prompt_tokens: u32 = req.inputs.iter().map(|i| i.len() as u32 / 4).sum();
        Ok(EmbedResponse {
            embeddings,
            usage: Some(LlmUsage {
                prompt_tokens,
                completion_tokens: 0,
                total_tokens: prompt_tokens,
            }),
            model: req.model,
            metadata: serde_json::Map::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ChatMessage;
    use futures::StreamExt;

    #[tokio::test]
    async fn echo_provider_streams_prompt_back() {
        let provider = MockProvider::new("test", MockMode::Echo);
        let req = ChatRequest::new("mock-model", vec![ChatMessage::user("hello world")]);
        let mut stream = provider.chat_stream(req);

        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event.expect("stream ok") {
                ChatEvent::Delta { text: t } => text.push_str(&t),
                ChatEvent::Finished { .. } => {}
            }
        }
        assert_eq!(text, "hello world");
    }

    #[tokio::test]
    async fn fixed_provider_returns_fixed_text() {
        let provider = MockProvider::new("test", MockMode::Fixed("canned response".into()));
        let req = ChatRequest::new("mock-model", vec![ChatMessage::user("anything")]);
        let mut stream = provider.chat_stream(req);

        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event.expect("stream ok") {
                ChatEvent::Delta { text: t } => text.push_str(&t),
                ChatEvent::Finished { .. } => {}
            }
        }
        assert_eq!(text, "canned response");
    }

    #[tokio::test]
    async fn mock_provider_embeds() {
        let provider = MockProvider::new("test", MockMode::Fixed("irrelevant".into()));
        let req = EmbedRequest::new("mock-model", vec!["hello".into()]);
        let resp = provider.embed(req).await.expect("embed ok");
        assert_eq!(resp.embeddings.len(), 1);
        assert_eq!(
            resp.embeddings[0].len(),
            16,
            "embedding must be 16-dimensional"
        );
        // Verify binary range (all values are 0.0 or 1.0)
        for &val in &resp.embeddings[0] {
            assert!(
                val == 0.0 || val == 1.0,
                "embedding value must be binary, got {val}"
            );
        }
        // Verify determinism: same input → same output
        let req2 = EmbedRequest::new("mock-model", vec!["hello".into()]);
        let resp2 = provider.embed(req2).await.expect("embed ok 2");
        assert_eq!(
            resp.embeddings[0], resp2.embeddings[0],
            "embeddings must be deterministic"
        );
    }

    #[tokio::test]
    async fn error_mode_emits_partial_then_error() {
        let provider = MockProvider::new(
            "test",
            MockMode::Error(LlmError::ProviderUnavailable("service down".into())),
        );
        let req = ChatRequest::new("mock-model", vec![ChatMessage::user("hello")]);
        let mut stream = provider.chat_stream(req);

        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }

        assert_eq!(events.len(), 2, "should emit exactly 2 events");
        assert!(matches!(events[0], Ok(ChatEvent::Delta { ref text }) if text == "partial"));
        assert!(matches!(events[1], Err(LlmError::ProviderUnavailable(_))));
    }

    #[tokio::test]
    async fn finished_event_has_usage() {
        let provider = MockProvider::new("test", MockMode::Fixed("hi".into()));
        let req = ChatRequest::new("mock-model", vec![ChatMessage::user("hi")]);
        let mut stream = provider.chat_stream(req);

        let mut found_usage = false;
        while let Some(event) = stream.next().await {
            if let ChatEvent::Finished { usage, .. } = event.expect("ok") {
                assert!(usage.is_some());
                found_usage = true;
            }
        }
        assert!(found_usage);
    }
}
