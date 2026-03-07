use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use tower::Service;
use tower::ServiceExt;
use tracing::Instrument;

use crate::config::DetailLevel;
use camel_api::{Body, BoxProcessor, CamelError, Exchange};

/// Returns a human-readable name for the body type variant.
fn body_type_name(body: &Body) -> &'static str {
    match body {
        Body::Empty => "empty",
        Body::Bytes(_) => "bytes",
        Body::Text(_) => "text",
        Body::Json(_) => "json",
        Body::Stream(_) => "stream",
    }
}

/// A processor wrapper that emits tracing spans for each step.
pub struct TracingProcessor {
    inner: BoxProcessor,
    route_id: String,
    step_id: String,
    step_index: usize,
    detail_level: DetailLevel,
}

impl TracingProcessor {
    /// Wrap a processor with tracing.
    pub fn new(
        inner: BoxProcessor,
        route_id: String,
        step_index: usize,
        detail_level: DetailLevel,
    ) -> Self {
        Self {
            inner,
            route_id,
            step_id: format!("step-{}", step_index),
            step_index,
            detail_level,
        }
    }
}

impl Service<Exchange> for TracingProcessor {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let start = Instant::now();
        let span = tracing::info_span!(
            target: "camel_tracer",
            "step",
            correlation_id = %exchange.correlation_id(),
            route_id = %self.route_id,
            step_id = %self.step_id,
            step_index = self.step_index,
            timestamp = %chrono::Utc::now().to_rfc3339(),
            duration_ms = tracing::field::Empty,
            status = tracing::field::Empty,
            headers_count = tracing::field::Empty,
            body_type = tracing::field::Empty,
            has_error = tracing::field::Empty,
            output_body_type = tracing::field::Empty,
            header_0 = tracing::field::Empty,
            header_1 = tracing::field::Empty,
            header_2 = tracing::field::Empty,
            error = tracing::field::Empty,
            error_type = tracing::field::Empty,
        );

        if self.detail_level >= DetailLevel::Medium {
            span.record("headers_count", exchange.input.headers.len() as u64);
            span.record("body_type", body_type_name(&exchange.input.body));
            span.record("has_error", exchange.has_error());
        }

        if self.detail_level >= DetailLevel::Full {
            for (i, (k, v)) in exchange.input.headers.iter().take(3).enumerate() {
                span.record(format!("header_{i}").as_str(), format!("{k}={v:?}"));
            }
        }

        let mut inner = self.inner.clone();
        let detail_level = self.detail_level.clone();

        Box::pin(
            async move {
                let result = inner.ready().await?.call(exchange).await;

                let duration_ms = start.elapsed().as_millis() as u64;
                tracing::Span::current().record("duration_ms", duration_ms);

                match &result {
                    Ok(ex) => {
                        tracing::Span::current().record("status", "success");
                        if detail_level >= DetailLevel::Medium {
                            tracing::Span::current()
                                .record("output_body_type", body_type_name(&ex.input.body));
                        }
                    }
                    Err(e) => {
                        tracing::Span::current().record("status", "error");
                        tracing::Span::current().record("error", e.to_string());
                        tracing::Span::current()
                            .record("error_type", std::any::type_name::<CamelError>());
                    }
                }

                result
            }
            .instrument(span),
        )
    }
}

impl Clone for TracingProcessor {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            route_id: self.route_id.clone(),
            step_id: self.step_id.clone(),
            step_index: self.step_index,
            detail_level: self.detail_level.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{IdentityProcessor, Message, Value};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_tracing_processor_minimal() {
        let inner = BoxProcessor::new(IdentityProcessor);
        let mut tracer =
            TracingProcessor::new(inner, "test-route".to_string(), 0, DetailLevel::Minimal);

        let exchange = Exchange::new(Message::default());
        let result = tracer.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_tracing_processor_medium_detail() {
        let inner = BoxProcessor::new(IdentityProcessor);
        let mut tracer =
            TracingProcessor::new(inner, "test-route".to_string(), 0, DetailLevel::Medium);

        let exchange = Exchange::new(Message::default());
        let result = tracer.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_tracing_processor_full_detail() {
        let inner = BoxProcessor::new(IdentityProcessor);
        let mut tracer =
            TracingProcessor::new(inner, "test-route".to_string(), 0, DetailLevel::Full);

        let mut exchange = Exchange::new(Message::default());
        exchange
            .input
            .headers
            .insert("test".to_string(), Value::String("value".into()));

        let result = tracer.ready().await.unwrap().call(exchange).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_tracing_processor_clone() {
        let inner = BoxProcessor::new(IdentityProcessor);
        let tracer =
            TracingProcessor::new(inner, "test-route".to_string(), 1, DetailLevel::Minimal);

        let mut cloned = tracer.clone();
        let exchange = Exchange::new(Message::default());
        let result = cloned.ready().await.unwrap().call(exchange).await;
        assert!(result.is_ok());
    }
}
