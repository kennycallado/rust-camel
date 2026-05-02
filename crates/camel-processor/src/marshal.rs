use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::body::Body;
use camel_api::data_format::DataFormat;
use camel_api::{CamelError, Exchange};

#[derive(Clone)]
pub struct MarshalService<P> {
    inner: P,
    format: Arc<dyn DataFormat>,
}

impl<P> MarshalService<P> {
    pub fn new(inner: P, format: Arc<dyn DataFormat>) -> Self {
        Self { inner, format }
    }
}

#[derive(Clone)]
pub struct UnmarshalService<P> {
    inner: P,
    format: Arc<dyn DataFormat>,
}

impl<P> UnmarshalService<P> {
    pub fn new(inner: P, format: Arc<dyn DataFormat>) -> Self {
        Self { inner, format }
    }
}

impl<P> Service<Exchange> for MarshalService<P>
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
        let body = std::mem::replace(&mut exchange.input.body, Body::Empty);
        let format = self.format.clone();
        match format.marshal(body) {
            Ok(new_body) => {
                exchange.input.body = new_body;
                let fut = self.inner.call(exchange);
                Box::pin(fut)
            }
            Err(e) => Box::pin(async move { Err(e) }),
        }
    }
}

impl<P> Service<Exchange> for UnmarshalService<P>
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
        let body = std::mem::replace(&mut exchange.input.body, Body::Empty);
        let format = self.format.clone();
        match format.unmarshal(body) {
            Ok(new_body) => {
                exchange.input.body = new_body;
                let fut = self.inner.call(exchange);
                Box::pin(fut)
            }
            Err(e) => Box::pin(async move { Err(e) }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_format::builtin_data_format;
    use camel_api::{IdentityProcessor, Message, Value};
    use serde_json::json;
    use std::sync::Arc;
    use tower::ServiceExt;

    struct FailingDataFormat;

    impl DataFormat for FailingDataFormat {
        fn name(&self) -> &str {
            "failing"
        }

        fn marshal(&self, _body: Body) -> Result<Body, CamelError> {
            Err(CamelError::ProcessorError("marshal-fail".to_string()))
        }

        fn unmarshal(&self, _body: Body) -> Result<Body, CamelError> {
            Err(CamelError::ProcessorError("unmarshal-fail".to_string()))
        }
    }

    #[tokio::test]
    async fn test_marshal_json_to_text() {
        let df = builtin_data_format("json").unwrap();
        let svc = MarshalService::new(IdentityProcessor, df);
        let exchange = Exchange::new(Message::new(Body::Json(json!({"a": 1}))));
        let result = svc.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Text(_)));
    }

    #[tokio::test]
    async fn test_unmarshal_text_to_json() {
        let df = builtin_data_format("json").unwrap();
        let svc = UnmarshalService::new(IdentityProcessor, df);
        let exchange = Exchange::new(Message::new(Body::Text(r#"{"b":2}"#.to_string())));
        let result = svc.oneshot(exchange).await.unwrap();
        assert!(matches!(result.input.body, Body::Json(_)));
    }

    #[tokio::test]
    async fn test_marshal_preserves_headers() {
        let df = builtin_data_format("json").unwrap();
        let svc = MarshalService::new(IdentityProcessor, df);
        let mut msg = Message::new(Body::Json(json!(1)));
        msg.set_header("keep", Value::Bool(true));
        let exchange = Exchange::new(msg);
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.header("keep"), Some(&Value::Bool(true)));
    }

    #[tokio::test]
    async fn test_unmarshal_invalid_returns_error() {
        let df = builtin_data_format("json").unwrap();
        let svc = UnmarshalService::new(IdentityProcessor, df);
        let exchange = Exchange::new(Message::new(Body::Text("bad".to_string())));
        let result = svc.oneshot(exchange).await;
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[tokio::test]
    async fn test_marshal_xml_json_to_text() {
        let df = builtin_data_format("xml").unwrap();
        let svc = MarshalService::new(IdentityProcessor, df);
        let exchange = Exchange::new(Message::new(Body::Json(json!({"root": {"c": "1"}}))));
        let result = svc.oneshot(exchange).await.unwrap();
        match result.input.body {
            Body::Text(s) => {
                assert_eq!(s, "<root><c>1</c></root>");
            }
            _ => panic!("expected Body::Text, got {:?}", result.input.body),
        }
    }

    #[tokio::test]
    async fn test_unmarshal_text_to_json_via_xml() {
        let df = builtin_data_format("xml").unwrap();
        let svc = UnmarshalService::new(IdentityProcessor, df);
        let exchange = Exchange::new(Message::new(Body::Text("<root><c/></root>".to_string())));
        let result = svc.oneshot(exchange).await.unwrap();
        match result.input.body {
            Body::Json(v) => {
                assert_eq!(v["root"]["c"], serde_json::Value::Null);
                assert_eq!(v, json!({"root": {"c": null}}));
            }
            _ => panic!("expected Body::Json, got {:?}", result.input.body),
        }
    }

    #[tokio::test]
    async fn test_marshal_error_propagates() {
        let svc = MarshalService::new(IdentityProcessor, Arc::new(FailingDataFormat));
        let exchange = Exchange::new(Message::new(Body::Text("x".to_string())));
        let result = svc.oneshot(exchange).await;
        assert!(matches!(result, Err(CamelError::ProcessorError(msg)) if msg == "marshal-fail"));
    }

    #[tokio::test]
    async fn test_unmarshal_error_propagates() {
        let svc = UnmarshalService::new(IdentityProcessor, Arc::new(FailingDataFormat));
        let exchange = Exchange::new(Message::new(Body::Text("x".to_string())));
        let result = svc.oneshot(exchange).await;
        assert!(matches!(result, Err(CamelError::ProcessorError(msg)) if msg == "unmarshal-fail"));
    }
}
