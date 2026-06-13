//! LlmProvider trait and request/response types.
//!
//! These types are Camel-shaped (not siumai-shaped).
//! The siumai adapter translates between these and siumai's types.

pub mod mock;
#[cfg(any(feature = "openai", feature = "ollama", feature = "all-providers"))]
pub mod siumai_adapter;

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::LlmError;

/// Provider abstraction for LLM chat and embedding operations.
///
/// Camel-shaped trait (not siumai-shaped). The siumai adapter
/// translates between these types and siumai's types.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Unique provider identifier (e.g., "openai", "ollama").
    fn id(&self) -> &str;

    /// Default model to use when none is specified in a request.
    fn default_model(&self) -> &str;

    /// Stream chat completions for the given request.
    fn chat_stream(&self, req: ChatRequest) -> BoxStream<'static, Result<ChatEvent, LlmError>>;

    /// Generate embeddings for the given inputs.
    async fn embed(&self, req: EmbedRequest) -> Result<EmbedResponse, LlmError>;

    /// Whether this provider supports embedding operations.
    fn supports_embed(&self) -> bool {
        true
    }
}

/// An event in a chat completion stream.
#[derive(Debug, Clone)]
pub enum ChatEvent {
    /// A partial text delta from the stream.
    Delta {
        /// The text chunk received.
        text: String,
    },
    /// The stream has finished.
    Finished {
        /// Usage statistics, if available.
        usage: Option<LlmUsage>,
        /// The model that generated the response.
        model: Option<String>,
        /// Why the stream finished.
        finish_reason: Option<FinishReason>,
        /// Provider-specific metadata.
        metadata: serde_json::Map<String, serde_json::Value>,
    },
}

/// Token usage statistics for an LLM operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LlmUsage {
    /// Tokens in the prompt.
    pub prompt_tokens: u32,
    /// Tokens in the completion.
    pub completion_tokens: u32,
    /// Total tokens used.
    pub total_tokens: u32,
}

/// Reason why a chat stream finished.
#[derive(Debug, Clone)]
pub enum FinishReason {
    /// Model stopped naturally.
    Stop,
    /// Response was truncated due to max tokens.
    Length,
    /// Model requested a tool call.
    ToolCall,
    /// Response was filtered by content policy.
    ContentFilter,
    /// Other reason.
    Other(String),
}

/// A request to a chat completion model.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// Model identifier (e.g., "gpt-4o", "llama-3-70b").
    pub model: String,
    /// Conversation messages.
    pub messages: Vec<ChatMessage>,
    /// Sampling temperature (0.0–2.0).
    pub temperature: Option<f64>,
    /// Maximum tokens to generate.
    pub max_tokens: Option<u32>,
    /// Sequences that stop generation.
    pub stop: Option<Vec<String>>,
    /// System prompt override.
    pub system_prompt: Option<String>,
    /// Additional provider-specific parameters.
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ChatRequest {
    /// Create a new chat request with the given model and messages.
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: model.into(),
            messages,
            temperature: None,
            max_tokens: None,
            stop: None,
            system_prompt: None,
            extra: serde_json::Map::new(),
        }
    }
}

/// A single message in a chat conversation.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    /// Role of the message author.
    pub role: ChatRole,
    /// Content of the message.
    pub content: String,
}

impl ChatMessage {
    /// Create a new user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            content: content.into(),
        }
    }
}

/// Role of a chat message author.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    /// System instruction.
    System,
    /// User message.
    User,
    /// Assistant response.
    Assistant,
}

/// A request to generate embeddings.
#[derive(Debug, Clone)]
pub struct EmbedRequest {
    /// Model identifier.
    pub model: String,
    /// Input texts to embed.
    pub inputs: Vec<String>,
    /// Additional provider-specific parameters.
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl EmbedRequest {
    /// Create a new embedding request.
    pub fn new(model: impl Into<String>, inputs: Vec<String>) -> Self {
        Self {
            model: model.into(),
            inputs,
            extra: serde_json::Map::new(),
        }
    }
}

/// Response from an embedding operation.
#[derive(Debug, Clone)]
pub struct EmbedResponse {
    /// Generated embedding vectors.
    pub embeddings: Vec<Vec<f32>>,
    /// Usage statistics, if available.
    pub usage: Option<LlmUsage>,
    /// Model used.
    pub model: String,
    /// Provider-specific metadata.
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_request_builder() {
        let req = ChatRequest::new("gpt-4o", vec![ChatMessage::user("hello")]);
        assert_eq!(req.model, "gpt-4o");
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, ChatRole::User);
    }
}
