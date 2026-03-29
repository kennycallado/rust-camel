//! Body coercion processor adapter.
//!
//! Wraps a [`BoxProcessor`] and calls [`body_converter::convert`] on the
//! exchange body before delegating. Follows the same pattern as `TracingProcessor`.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;
use tower::ServiceExt;

use camel_api::{BodyType, BoxProcessor, CamelError, Exchange, body_converter};

/// Wraps a processor and coerces the exchange body to the required type before
/// calling the inner processor.
///
/// Constructed only when `contract` is `Some` — never wrap with `None`.
/// Use [`wrap_if_needed`] as the factory.
#[derive(Clone)]
pub struct BodyCoercingProcessor {
    inner: BoxProcessor,
    contract: BodyType,
}

impl BodyCoercingProcessor {
    /// Wrap `inner` with automatic body coercion to `contract`.
    pub fn new(inner: BoxProcessor, contract: BodyType) -> Self {
        Self { inner, contract }
    }
}

impl Service<Exchange> for BodyCoercingProcessor {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let contract = self.contract;
        let mut inner = self.inner.clone();

        Box::pin(async move {
            let body = std::mem::replace(&mut exchange.input.body, camel_api::body::Body::Empty);
            exchange.input.body = body_converter::convert(body, contract).map_err(|e| {
                CamelError::TypeConversionFailed(format!("body coercion failed: {e}"))
            })?;
            inner.ready().await?.call(exchange).await
        })
    }
}

/// Return `processor` wrapped with body coercion if `contract` is `Some`,
/// or the processor unchanged if `contract` is `None` (zero overhead).
pub fn wrap_if_needed(processor: BoxProcessor, contract: Option<BodyType>) -> BoxProcessor {
    match contract {
        Some(t) => BoxProcessor::new(BodyCoercingProcessor::new(processor, t)),
        None => processor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Body, BoxProcessorExt, Exchange, Message};
    use serde_json::json;
    use tower::ServiceExt;

    // Helper: identity processor
    fn identity() -> BoxProcessor {
        BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
    }

    #[tokio::test]
    async fn coerces_json_to_text() {
        let inner = identity();
        let mut processor = BodyCoercingProcessor::new(inner, BodyType::Text);

        let mut exchange = Exchange::new(Message::default());
        exchange.input.body = Body::Json(json!({"key": "value"}));

        let result = processor
            .ready()
            .await
            .unwrap()
            .call(exchange)
            .await
            .unwrap();
        assert!(matches!(result.input.body, Body::Text(_)));
    }

    #[tokio::test]
    async fn coerces_text_to_bytes() {
        let inner = identity();
        let mut processor = BodyCoercingProcessor::new(inner, BodyType::Bytes);

        let mut exchange = Exchange::new(Message::default());
        exchange.input.body = Body::Text("hello".into());

        let result = processor
            .ready()
            .await
            .unwrap()
            .call(exchange)
            .await
            .unwrap();
        assert!(matches!(result.input.body, Body::Bytes(_)));
    }

    #[tokio::test]
    async fn returns_error_on_failed_coercion() {
        let inner = identity();
        let mut processor = BodyCoercingProcessor::new(inner, BodyType::Json);

        let mut exchange = Exchange::new(Message::default());
        // "not json" cannot be converted to JSON
        exchange.input.body = Body::Text("not json".into());

        let result = processor.ready().await.unwrap().call(exchange).await;
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[tokio::test]
    async fn empty_to_text_fails_with_coercion_error() {
        let inner = identity();
        let mut processor = BodyCoercingProcessor::new(inner, BodyType::Text);

        let exchange = Exchange::new(Message::default()); // Body::Empty

        let result = processor.ready().await.unwrap().call(exchange).await;
        assert!(
            matches!(result, Err(CamelError::TypeConversionFailed(ref msg)) if msg.contains("body coercion failed")),
            "expected TypeConversionFailed with 'body coercion failed', got: {:?}",
            result
        );
    }

    #[test]
    fn wrap_if_needed_with_none_returns_same_type_id() {
        let inner = BoxProcessor::from_fn(|mut ex: Exchange| {
            Box::pin(async move {
                ex.input.body = Body::Text("marker".into());
                Ok(ex)
            })
        });
        let wrapped = wrap_if_needed(inner, None);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::Empty;
        let result = rt.block_on(async { tower::ServiceExt::oneshot(wrapped, ex).await });
        assert!(matches!(result.unwrap().input.body, Body::Text(ref s) if s == "marker"));
    }

    #[tokio::test]
    async fn wrap_if_needed_with_some_wraps_processor() {
        let inner = identity();
        let wrapped = wrap_if_needed(inner, Some(BodyType::Text));

        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::Json(json!(42));

        let result = tower::ServiceExt::oneshot(wrapped, ex).await.unwrap();
        assert!(matches!(result.input.body, Body::Text(ref s) if s == "42"));
    }
}
