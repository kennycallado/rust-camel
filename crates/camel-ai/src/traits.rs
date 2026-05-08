use async_trait::async_trait;
use camel_api::CamelError;

use crate::types::{ChatRequest, ChatResponse, VectorHit, VectorItem};

/// Async interface for chat/completion models. Vendor-neutral.
#[async_trait]
pub trait ChatModel: Send + Sync {
    async fn complete(&self, req: ChatRequest) -> Result<ChatResponse, CamelError>;
}

/// Vec<String> input is batch-ready for Phase 2. Phase 1: single-element vec.
#[async_trait]
pub trait EmbeddingModel: Send + Sync {
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, CamelError>;
}

/// Async interface for vector store backends. Vendor-neutral.
#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn upsert(&self, items: Vec<VectorItem>) -> Result<(), CamelError>;
    async fn search(&self, query: Vec<f32>, top_k: usize) -> Result<Vec<VectorHit>, CamelError>;
}
