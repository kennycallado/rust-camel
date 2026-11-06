use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::body::Body;
use camel_api::data_format::DataFormat;
use camel_api::{CamelError, Exchange};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MarshalSide {
    Step,
    Response,
}

#[derive(Clone)]
pub struct MarshalService<P> {
    inner: P,
    format: Arc<dyn DataFormat>,
    side: MarshalSide,
}

impl<P> MarshalService<P> {
    pub fn new(inner: P, format: Arc<dyn DataFormat>) -> Self {
        Self {
            inner,
            format,
            side: MarshalSide::Step,
        }
    }

    /// Build a marshal step for the response side of an exchange.
    ///
    /// Response-side marshal failures are deliberately wrapped in
    /// [`CamelError::ProcessorErrorWithSource`] so the HTTP finalizer maps the
    /// failure to 500. This follows the ADR-0096 blame rule: conversion
    /// failures default to 500 unless the failing step's input is by definition
    /// the request. The binding-injected unmarshal is that seam; it
    /// reclassifies [`CamelError::TypeConversionFailed`] to
    /// [`CamelError::ValidationError`] (HTTP 400) via
    /// [`UnmarshalService::for_request_binding`]. The source chain preserves
    /// the original error.
    pub fn for_response(inner: P, format: Arc<dyn DataFormat>) -> Self {
        Self {
            inner,
            format,
            side: MarshalSide::Response,
        }
    }
}

const _: fn() = || {
    fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    assert_send_sync_static::<CamelError>();
};

#[derive(Clone)]
pub struct UnmarshalService<P> {
    inner: P,
    format: Arc<dyn DataFormat>,
    request_borne: bool,
}

impl<P> UnmarshalService<P> {
    pub fn new(inner: P, format: Arc<dyn DataFormat>) -> Self {
        Self {
            inner,
            format,
            request_borne: false,
        }
    }

    /// Build an unmarshal step for a REST binding's request body (ADR-0096).
    ///
    /// The binding-injected unmarshal is the request-borne seam: its failing
    /// input is by definition the client request. A format
    /// [`CamelError::TypeConversionFailed`] is reclassified to
    /// [`CamelError::ValidationError`] so consumers map it to 400. Every other
    /// error — other format variants and inner-service failures — passes
    /// through unchanged. User-authored `unmarshal` steps use [`Self::new`] and
    /// inherit the 500 default.
    pub fn for_request_binding(inner: P, format: Arc<dyn DataFormat>) -> Self {
        Self {
            inner,
            format,
            request_borne: true,
        }
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
        match format.marshal_in_exchange(&mut exchange, body) {
            Ok(new_body) => {
                exchange.input.body = new_body;
                let fut = self.inner.call(exchange);
                Box::pin(fut)
            }
            Err(e) => {
                if self.side == MarshalSide::Response {
                    let format_name = self.format.name().to_string();
                    Box::pin(async move {
                        Err(CamelError::ProcessorErrorWithSource(
                            format!("response marshal failed ({format_name})"),
                            Arc::new(e),
                        ))
                    })
                } else {
                    Box::pin(async move { Err(e) })
                }
            }
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
        match format.unmarshal_in_exchange(&mut exchange, body) {
            Ok(new_body) => {
                exchange.input.body = new_body;
                let fut = self.inner.call(exchange);
                Box::pin(fut)
            }
            Err(e) => {
                // ADR-0096: only the binding seam's format conversion failure is
                // request-borne. Inner-service errors bypass this mapping.
                let e = if self.request_borne {
                    match e {
                        CamelError::TypeConversionFailed(msg) => {
                            CamelError::ValidationError(format!("request body rejected: {msg}"))
                        }
                        other => other,
                    }
                } else {
                    e
                };
                Box::pin(async move { Err(e) })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_format::JsonDataFormat;
    use crate::data_format::XmlDataFormat;
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

    struct ConversionFailingDataFormat;

    impl DataFormat for ConversionFailingDataFormat {
        fn name(&self) -> &str {
            "conversion-failing"
        }

        fn marshal(&self, body: Body) -> Result<Body, CamelError> {
            Ok(body)
        }

        fn unmarshal(&self, _body: Body) -> Result<Body, CamelError> {
            Err(CamelError::TypeConversionFailed("boom".to_string()))
        }
    }

    struct RouteFailingDataFormat;

    impl DataFormat for RouteFailingDataFormat {
        fn name(&self) -> &str {
            "route-failing"
        }

        fn marshal(&self, body: Body) -> Result<Body, CamelError> {
            Ok(body)
        }

        fn unmarshal(&self, _body: Body) -> Result<Body, CamelError> {
            Err(CamelError::RouteError("x".to_string()))
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

    const MARSHAL_HOOK_KEY: &str = "TestMarshalHookCalled";

    struct HeaderWritingMarshalFormat;

    impl DataFormat for HeaderWritingMarshalFormat {
        fn name(&self) -> &str {
            "header-writer-marshal"
        }
        fn marshal(&self, body: Body) -> Result<Body, CamelError> {
            Ok(body)
        }
        fn unmarshal(&self, body: Body) -> Result<Body, CamelError> {
            Ok(body)
        }

        fn marshal_in_exchange(
            &self,
            exchange: &mut Exchange,
            body: Body,
        ) -> Result<Body, CamelError> {
            exchange
                .input
                .headers
                .insert(MARSHAL_HOOK_KEY.to_string(), serde_json::Value::Bool(true));
            Ok(body)
        }
    }

    #[tokio::test]
    async fn test_marshal_service_propagates_exchange_to_hook() {
        let svc = MarshalService::new(IdentityProcessor, Arc::new(HeaderWritingMarshalFormat));
        let ex = Exchange::new(Message::new(Body::Json(serde_json::json!({}))));
        let result = svc.oneshot(ex).await.unwrap();
        assert_eq!(
            result.input.headers.get(MARSHAL_HOOK_KEY),
            Some(&serde_json::Value::Bool(true))
        );
    }

    const UNMARSHAL_HOOK_KEY: &str = "TestUnmarshalHookCalled";

    struct HeaderWritingUnmarshalFormat;

    impl DataFormat for HeaderWritingUnmarshalFormat {
        fn name(&self) -> &str {
            "header-writer-unmarshal"
        }
        fn marshal(&self, body: Body) -> Result<Body, CamelError> {
            Ok(body)
        }
        fn unmarshal(&self, body: Body) -> Result<Body, CamelError> {
            Ok(body)
        }

        fn unmarshal_in_exchange(
            &self,
            exchange: &mut Exchange,
            body: Body,
        ) -> Result<Body, CamelError> {
            exchange.input.headers.insert(
                UNMARSHAL_HOOK_KEY.to_string(),
                serde_json::Value::Bool(true),
            );
            Ok(body)
        }
    }

    #[tokio::test]
    async fn test_unmarshal_service_propagates_exchange_to_hook() {
        let svc = UnmarshalService::new(IdentityProcessor, Arc::new(HeaderWritingUnmarshalFormat));
        let ex = Exchange::new(Message::new(Body::Json(serde_json::json!({}))));
        let result = svc.oneshot(ex).await.unwrap();
        assert_eq!(
            result.input.headers.get(UNMARSHAL_HOOK_KEY),
            Some(&serde_json::Value::Bool(true))
        );
    }

    #[tokio::test]
    async fn for_response_wraps_marshal_error_with_source() {
        let svc =
            MarshalService::for_response(IdentityProcessor, Arc::new(XmlDataFormat::default()));
        let exchange = Exchange::new(Message::new(Body::Json(json!({"a": 1, "b": 2}))));
        let result = svc.oneshot(exchange).await;
        match result {
            Err(CamelError::ProcessorErrorWithSource(msg, source)) => {
                assert!(
                    msg.contains("response marshal failed"),
                    "message must name the response-side failure, got {msg:?}"
                );
                assert!(
                    matches!(
                        source.downcast_ref::<CamelError>(),
                        Some(CamelError::TypeConversionFailed(_))
                    ),
                    "source must preserve the original TypeConversionFailed, got {source:?}"
                );
            }
            other => panic!("expected ProcessorErrorWithSource, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn new_keeps_type_conversion_failure() {
        let svc = MarshalService::new(IdentityProcessor, Arc::new(XmlDataFormat::default()));
        let exchange = Exchange::new(Message::new(Body::Json(json!({"a": 1, "b": 2}))));
        let result = svc.oneshot(exchange).await;
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[tokio::test]
    async fn for_response_passes_success_through() {
        let svc =
            MarshalService::for_response(IdentityProcessor, Arc::new(JsonDataFormat::default()));
        let exchange = Exchange::new(Message::new(Body::Json(json!({"a": 1}))));
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body, Body::Text("{\"a\":1}".to_string()));
    }

    #[tokio::test]
    async fn request_binding_reclassifies_format_conversion_failure() {
        let svc = UnmarshalService::for_request_binding(
            IdentityProcessor,
            Arc::new(ConversionFailingDataFormat),
        );
        let exchange = Exchange::new(Message::new(Body::Text("x".to_string())));
        let result = svc.oneshot(exchange).await;
        match result {
            Err(CamelError::ValidationError(msg)) => {
                assert!(
                    msg.contains("request body rejected: boom"),
                    "message must name the request-borne rejection, got {msg:?}"
                );
            }
            other => panic!("expected ValidationError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn request_binding_passes_inner_conversion_failure_through() {
        let inner = tower::service_fn(|_ex: Exchange| async {
            Err::<Exchange, CamelError>(CamelError::TypeConversionFailed("inner".to_string()))
        });
        let svc = UnmarshalService::for_request_binding(inner, Arc::new(JsonDataFormat::default()));
        let exchange = Exchange::new(Message::new(Body::Text(r#"{"a":1}"#.to_string())));
        let result = svc.oneshot(exchange).await;
        assert!(
            matches!(result, Err(CamelError::TypeConversionFailed(msg)) if msg == "inner"),
            "inner conversion failures must bypass the reclassification boundary"
        );
    }

    #[tokio::test]
    async fn request_binding_passes_other_format_variants_through() {
        let svc = UnmarshalService::for_request_binding(
            IdentityProcessor,
            Arc::new(RouteFailingDataFormat),
        );
        let exchange = Exchange::new(Message::new(Body::Text("x".to_string())));
        let result = svc.oneshot(exchange).await;
        assert!(
            matches!(result, Err(CamelError::RouteError(msg)) if msg == "x"),
            "only TypeConversionFailed is reclassified"
        );
    }

    #[tokio::test]
    async fn step_unmarshal_keeps_conversion_failure() {
        let svc = UnmarshalService::new(IdentityProcessor, Arc::new(ConversionFailingDataFormat));
        let exchange = Exchange::new(Message::new(Body::Text("x".to_string())));
        let result = svc.oneshot(exchange).await;
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(msg)) if msg == "boom"));
    }
}
