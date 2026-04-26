use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::body::Body;
use camel_api::stream_cache::StreamCacheConfig;
use camel_api::{CamelError, Exchange};
use tower::ServiceExt;

#[derive(Clone)]
pub struct StreamCacheService<P> {
    inner: P,
    config: StreamCacheConfig,
}

impl<P> StreamCacheService<P> {
    pub fn new(inner: P, config: StreamCacheConfig) -> Self {
        Self { inner, config }
    }
}

impl<P> Service<Exchange> for StreamCacheService<P>
where
    P: Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + 'static,
    P::Future: Send,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let threshold = self.config.threshold;
        let body = std::mem::replace(&mut exchange.input.body, Body::Empty);
        match body {
            Body::Stream(_) => {
                let inner = self.inner.clone();
                Box::pin(async move {
                    let bytes = body.into_bytes(threshold).await?;
                    exchange.input.body = Body::Bytes(bytes);
                    inner.oneshot(exchange).await
                })
            }
            _ => {
                exchange.input.body = body;
                let fut = self.inner.call(exchange);
                Box::pin(fut)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use camel_api::body::{StreamBody, StreamMetadata};
    use camel_api::{IdentityProcessor, Message};
    use futures::stream;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    fn make_stream_body(data: Vec<u8>) -> Body {
        let chunks: Vec<Result<Bytes, CamelError>> = vec![Ok(Bytes::from(data))];
        let stream = stream::iter(chunks);
        Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata::default(),
        })
    }

    #[tokio::test]
    async fn test_stream_cache_materializes_stream() {
        let svc = StreamCacheService::new(IdentityProcessor, StreamCacheConfig::default());
        let exchange = Exchange::new(Message::new(make_stream_body(b"hello".to_vec())));
        let result = svc.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Bytes(_)));
        match &result.input.body {
            Body::Bytes(b) => assert_eq!(b.as_ref(), b"hello"),
            _ => panic!("expected Bytes"),
        }
    }

    #[tokio::test]
    async fn test_stream_cache_passes_through_text() {
        let svc = StreamCacheService::new(IdentityProcessor, StreamCacheConfig::default());
        let exchange = Exchange::new(Message::new(Body::Text("unchanged".to_string())));
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body, Body::Text("unchanged".to_string()));
    }

    #[tokio::test]
    async fn test_stream_cache_passes_through_bytes() {
        let svc = StreamCacheService::new(IdentityProcessor, StreamCacheConfig::default());
        let exchange = Exchange::new(Message::new(Body::Bytes(Bytes::from_static(b"data"))));
        let result = svc.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Bytes(_)));
    }

    #[tokio::test]
    async fn test_stream_cache_passes_through_json() {
        let svc = StreamCacheService::new(IdentityProcessor, StreamCacheConfig::default());
        let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({"k": 1}))));
        let result = svc.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Json(_)));
    }

    #[tokio::test]
    async fn test_stream_cache_exceeds_threshold() {
        let svc = StreamCacheService::new(IdentityProcessor, StreamCacheConfig::new(5));
        let exchange = Exchange::new(Message::new(make_stream_body(b"hello world".to_vec())));
        let result = svc.oneshot(exchange).await;
        assert!(matches!(result, Err(CamelError::StreamLimitExceeded(_))));
    }

    #[tokio::test]
    async fn test_stream_cache_preserves_headers() {
        use camel_api::Value;
        let svc = StreamCacheService::new(IdentityProcessor, StreamCacheConfig::default());
        let mut msg = Message::new(make_stream_body(b"data".to_vec()));
        msg.set_header("x-test", Value::String("kept".into()));
        let exchange = Exchange::new(msg);
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header("x-test"),
            Some(&Value::String("kept".into()))
        );
    }

    #[tokio::test]
    async fn test_e2e_stream_cache_then_convert_body_to() {
        use crate::ConvertBodyTo;
        use camel_api::body_converter::BodyType;

        let inner = ConvertBodyTo::new(IdentityProcessor, BodyType::Text);
        let svc = StreamCacheService::new(inner, StreamCacheConfig::default());
        let exchange = Exchange::new(Message::new(make_stream_body(b"hello".to_vec())));
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body, Body::Text("hello".to_string()));
    }

    #[tokio::test]
    async fn test_e2e_stream_cache_then_unmarshal_json() {
        use crate::UnmarshalService;
        use crate::data_format::builtin_data_format;

        let df = builtin_data_format("json").unwrap();
        let inner = UnmarshalService::new(IdentityProcessor, df);
        let svc = StreamCacheService::new(inner, StreamCacheConfig::default());
        let exchange = Exchange::new(Message::new(make_stream_body(
            br#"{"key":"value"}"#.to_vec(),
        )));
        let result = svc.oneshot(exchange).await.unwrap();
        match &result.input.body {
            Body::Json(v) => assert_eq!(v["key"], "value"),
            other => panic!("expected Json, got {:?}", other),
        }
    }
}
