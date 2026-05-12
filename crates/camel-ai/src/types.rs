use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Header name carrying embedding vector (JSON Value array) between steps.
/// Lives here (not in components) so both camel-component-embedding and
/// camel-component-vector can import without cross-component coupling.
pub const HEADER_CAMEL_AI_EMBEDDING: &str = "CamelAiEmbedding";

pub const HEADER_CAMEL_AI_PROVIDER: &str = "CamelAiProvider";
pub const HEADER_CAMEL_AI_MODEL: &str = "CamelAiModel";
pub const HEADER_CAMEL_AI_OPERATION: &str = "CamelAiOperation";
pub const HEADER_CAMEL_AI_LATENCY_MS: &str = "CamelAiLatencyMs";
pub const HEADER_CAMEL_AI_PROMPT_TOKENS: &str = "CamelAiPromptTokens";
pub const HEADER_CAMEL_AI_COMPLETION_TOKENS: &str = "CamelAiCompletionTokens";
pub const HEADER_CAMEL_AI_TOTAL_TOKENS: &str = "CamelAiTotalTokens";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

/// model is owned by the adapter config, not the request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub think: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub content: String,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorItem {
    pub id: String,
    pub vector: Vec<f32>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorHit {
    pub id: String,
    pub score: f32,
    pub payload: serde_json::Value,
}

pub fn set_ai_headers(
    headers: &mut HashMap<String, serde_json::Value>,
    provider: &str,
    model: &str,
    operation: &str,
    latency_ms: u64,
    usage: Option<&TokenUsage>,
) {
    headers.insert(HEADER_CAMEL_AI_PROVIDER.into(), serde_json::Value::String(provider.into()));
    headers.insert(HEADER_CAMEL_AI_MODEL.into(), serde_json::Value::String(model.into()));
    headers.insert(HEADER_CAMEL_AI_OPERATION.into(), serde_json::Value::String(operation.into()));
    headers.insert(HEADER_CAMEL_AI_LATENCY_MS.into(), serde_json::json!(latency_ms));
    if let Some(u) = usage {
        headers.insert(HEADER_CAMEL_AI_PROMPT_TOKENS.into(), serde_json::json!(u.prompt_tokens));
        headers.insert(HEADER_CAMEL_AI_COMPLETION_TOKENS.into(), serde_json::json!(u.completion_tokens));
        headers.insert(HEADER_CAMEL_AI_TOTAL_TOKENS.into(), serde_json::json!(u.total_tokens));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_role_serde_round_trip() {
        // System → "system" → System
        let role = ChatRole::System;
        let serialized = serde_json::to_string(&role).unwrap();
        assert_eq!(serialized, "\"system\"");
        let deserialized: ChatRole = serde_json::from_str(&serialized).unwrap();
        assert!(matches!(deserialized, ChatRole::System));

        // User → "user" → User
        let role = ChatRole::User;
        let serialized = serde_json::to_string(&role).unwrap();
        assert_eq!(serialized, "\"user\"");
        let deserialized: ChatRole = serde_json::from_str(&serialized).unwrap();
        assert!(matches!(deserialized, ChatRole::User));

        // Assistant → "assistant" → Assistant
        let role = ChatRole::Assistant;
        let serialized = serde_json::to_string(&role).unwrap();
        assert_eq!(serialized, "\"assistant\"");
        let deserialized: ChatRole = serde_json::from_str(&serialized).unwrap();
        assert!(matches!(deserialized, ChatRole::Assistant));
    }

    #[test]
    fn header_constant_value() {
        assert_eq!(HEADER_CAMEL_AI_EMBEDDING, "CamelAiEmbedding");
    }
}
