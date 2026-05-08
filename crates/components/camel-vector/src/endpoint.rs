use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use camel_ai::{HEADER_CAMEL_AI_EMBEDDING, VectorItem, VectorStore};
use camel_component_api::{
    Body, BoxProcessor, CamelError, Consumer, Endpoint, Exchange, ProducerContext,
};
use tower::Service;

pub struct VectorEndpoint {
    pub uri: String,
    pub operation: String,
    pub store: Arc<dyn VectorStore>,
    pub top_k: usize,
}

impl Endpoint for VectorEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::RouteError(
            "vector: component is producer-only".into(),
        ))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        match self.operation.as_str() {
            "upsert" => Ok(BoxProcessor::new(VectorUpsertProducer {
                store: Arc::clone(&self.store),
            })),
            "search" => Ok(BoxProcessor::new(VectorSearchProducer {
                store: Arc::clone(&self.store),
                top_k: self.top_k,
            })),
            op => Err(CamelError::RouteError(format!(
                "vector: unknown operation '{op}'"
            ))),
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn read_embedding(exchange: &Exchange) -> Result<Vec<f32>, CamelError> {
    serde_json::from_value(
        exchange
            .input
            .headers
            .get(HEADER_CAMEL_AI_EMBEDDING)
            .ok_or_else(|| {
                CamelError::RouteError(format!("missing header '{HEADER_CAMEL_AI_EMBEDDING}'"))
            })?
            .clone(),
    )
    .map_err(|e| CamelError::RouteError(format!("parse embedding: {e}")))
}

// ── Upsert Producer ───────────────────────────────────────────────────────────

#[derive(Clone)]
struct VectorUpsertProducer {
    store: Arc<dyn VectorStore>,
}

impl Service<Exchange> for VectorUpsertProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            let vector = read_embedding(&exchange)?;

            let id = exchange.correlation_id.clone();
            let text = exchange.input.body.as_text().unwrap_or("").to_string();
            let payload = serde_json::json!({ "text": text });

            store
                .upsert(vec![VectorItem {
                    id,
                    vector,
                    payload,
                }])
                .await?;
            Ok(exchange)
        })
    }
}

// ── Search Producer ───────────────────────────────────────────────────────────

#[derive(Clone)]
struct VectorSearchProducer {
    store: Arc<dyn VectorStore>,
    top_k: usize,
}

impl Service<Exchange> for VectorSearchProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let store = Arc::clone(&self.store);
        let top_k = self.top_k;
        Box::pin(async move {
            let query = read_embedding(&exchange)?;

            let hits = store.search(query, top_k).await?;

            let json_body = serde_json::to_string(&hits)
                .map_err(|e| CamelError::RouteError(format!("serialize hits: {e}")))?;
            exchange.input.body = Body::Text(json_body);
            Ok(exchange)
        })
    }
}
