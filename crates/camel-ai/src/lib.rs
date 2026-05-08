pub mod adapters;
pub mod traits;
pub mod types;

pub use adapters::{OllamaAdapter, OllamaConfig, OpenAiAdapter, OpenAiConfig, QdrantConfig, QdrantStore};
pub use traits::{ChatModel, EmbeddingModel, VectorStore};
pub use types::{
    ChatMessage, ChatRequest, ChatResponse, ChatRole, TokenUsage, VectorHit, VectorItem,
    HEADER_CAMEL_AI_EMBEDDING,
};
