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
#[non_exhaustive]
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
    /// An intermediate tool call emitted during streaming.
    ToolCall {
        /// Tool call identifier.
        id: String,
        /// Name of the tool to invoke.
        name: String,
        /// JSON arguments for the tool.
        arguments: String,
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

/// Definition of a tool the model may call.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolDefinition {
    /// Name of the tool (e.g., "get_weather").
    pub name: String,
    /// Description of what the tool does.
    pub description: String,
    /// JSON Schema parameters for the tool.
    pub parameters: serde_json::Map<String, serde_json::Value>,
}

/// Controls how the model chooses which tool to call.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ToolChoice {
    /// Model may decide to call zero or more tools.
    Auto,
    /// Model must not call any tool.
    None,
    /// Model must call the named tool.
    Specific(String),
}

/// A tool call emitted by the model as part of an assistant message.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmittedToolCall {
    /// Unique identifier for this tool call.
    pub id: String,
    /// Name of the tool to invoke.
    pub name: String,
    /// JSON string of arguments for the tool.
    pub arguments: String,
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
    /// Tools available for the model to call.
    pub tools: Vec<ToolDefinition>,
    /// Controls tool selection behaviour.
    pub tool_choice: Option<ToolChoice>,
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
            tools: Vec::new(),
            tool_choice: None,
            extra: serde_json::Map::new(),
        }
    }
}

/// A single message in a chat conversation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatMessage {
    /// Role of the message author.
    pub role: ChatRole,
    /// Content of the message.
    pub content: String,
    /// Tool calls made by the assistant, if any.
    pub tool_calls: Option<Vec<EmittedToolCall>>,
}

impl ChatMessage {
    /// Create a new user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            content: content.into(),
            tool_calls: None,
        }
    }
}

/// Role of a chat message author.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum ChatRole {
    /// System instruction.
    System,
    /// User message.
    User,
    /// Assistant response.
    Assistant,
    /// Tool result message carrying the output of a tool call.
    Tool {
        /// The identifier of the tool call this result belongs to.
        tool_call_id: String,
    },
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

    #[test]
    fn chat_request_accepts_tools() {
        let tool = ToolDefinition {
            name: "get_weather".into(),
            description: "Get weather for a city".into(),
            parameters: serde_json::Map::new(),
        };
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage::user("what's the weather?")],
            temperature: None,
            max_tokens: None,
            stop: None,
            system_prompt: None,
            extra: serde_json::Map::new(),
            tools: vec![tool],
            tool_choice: Some(ToolChoice::Auto),
        };
        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.tools[0].name, "get_weather");
        assert_eq!(req.tool_choice, Some(ToolChoice::Auto));
    }

    #[test]
    fn tool_message_carries_tool_call_id() {
        let msg = ChatMessage {
            role: ChatRole::Tool {
                tool_call_id: "call_123".into(),
            },
            content: "weather result".into(),
            tool_calls: None,
        };
        match &msg.role {
            ChatRole::Tool { tool_call_id } => assert_eq!(tool_call_id, "call_123"),
            _ => panic!("expected Tool role"),
        }
    }

    #[test]
    fn assistant_message_carries_prior_tool_calls() {
        let tool_call = EmittedToolCall {
            id: "call_123".into(),
            name: "get_weather".into(),
            arguments: r#"{"city":"London"}"#.into(),
        };
        let msg = ChatMessage {
            role: ChatRole::Assistant,
            content: "I'll check the weather".into(),
            tool_calls: Some(vec![tool_call]),
        };
        assert_eq!(msg.tool_calls.as_ref().unwrap().len(), 1);
        assert_eq!(msg.tool_calls.as_ref().unwrap()[0].id, "call_123");
    }
}
