pub mod adapters;
pub mod traits;
pub mod types;
pub mod uri;

pub use adapters::{
    OllamaAdapter, OllamaConfig, OpenAiAdapter, OpenAiConfig, QdrantConfig, QdrantStore,
};
pub use traits::{ChatModel, EmbeddingModel, VectorStore};
pub use types::{
    ChatMessage, ChatRequest, ChatResponse, ChatRole, HEADER_CAMEL_AI_COMPLETION_TOKENS,
    HEADER_CAMEL_AI_EMBEDDING, HEADER_CAMEL_AI_LATENCY_MS, HEADER_CAMEL_AI_MODEL,
    HEADER_CAMEL_AI_OPERATION, HEADER_CAMEL_AI_PROMPT_TOKENS, HEADER_CAMEL_AI_PROVIDER,
    HEADER_CAMEL_AI_TOTAL_TOKENS, TokenUsage, VectorHit, VectorItem, set_ai_headers,
};
pub use uri::{AiCapability, AiModelUri, AiProvider, resolve_chat_model, resolve_embedding_model};
