use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use camel_ai::{EmbeddingModel, HEADER_CAMEL_AI_EMBEDDING};
use camel_component_api::{
    BoxProcessor, CamelError, Consumer, Endpoint, Exchange, ProducerContext,
};
use tower::Service;

pub struct EmbeddingEndpoint {
    pub uri: String,
    pub model: Arc<dyn EmbeddingModel>,
}

impl Endpoint for EmbeddingEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::RouteError(
            "embedding: component is producer-only".into(),
        ))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(EmbeddingProducer {
            model: Arc::clone(&self.model),
        }))
    }
}

#[derive(Clone)]
struct EmbeddingProducer {
    model: Arc<dyn EmbeddingModel>,
}

impl Service<Exchange> for EmbeddingProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let model = Arc::clone(&self.model);
        Box::pin(async move {
            let text = exchange
                .input
                .body
                .as_text()
                .ok_or_else(|| CamelError::TypeConversionFailed("body is not text".into()))?
                .to_string();

            let vectors = model.embed(vec![text]).await?;
            let vector = vectors.into_iter().next().ok_or_else(|| {
                CamelError::RouteError("embedding model returned empty result".into())
            })?;

            let vector_val = serde_json::to_value(&vector)
                .map_err(|e| CamelError::RouteError(format!("serialize embedding: {e}")))?;
            exchange
                .input
                .headers
                .insert(HEADER_CAMEL_AI_EMBEDDING.into(), vector_val);

            Ok(exchange)
        })
    }
}
