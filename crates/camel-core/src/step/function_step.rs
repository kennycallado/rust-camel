use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use camel_api::{
    CamelError, Exchange, ExchangePatch, FunctionDefinition, FunctionId, FunctionInvocationError,
    FunctionInvoker, PatchBody,
};
use tower::Service;

#[derive(Clone)]
pub struct FunctionStep {
    definition: FunctionDefinition,
    invoker: Arc<dyn FunctionInvoker>,
}

impl FunctionStep {
    pub fn new(invoker: Arc<dyn FunctionInvoker>, definition: FunctionDefinition) -> Self {
        Self { definition, invoker }
    }
}

impl Service<Exchange> for FunctionStep {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut ex: Exchange) -> Self::Future {
        let invoker = Arc::clone(&self.invoker);
        let id = self.definition.id.clone();
        let timeout_ms = self.definition.timeout_ms;
        Box::pin(async move {
            let result = tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                invoker.invoke(&id, &ex),
            )
            .await
            .map_err(|_| {
                CamelError::ProcessorError(format!(
                    "function:timeout: {} timed out after {}ms",
                    id.0, timeout_ms
                ))
            })?;
            let patch = result.map_err(|e| map_invocation_error(e, &id))?;
            apply_patch(&mut ex, patch);
            Ok(ex)
        })
    }
}

fn map_invocation_error(err: FunctionInvocationError, id: &FunctionId) -> CamelError {
    match err {
        FunctionInvocationError::UserError { message, .. } => {
            CamelError::ProcessorError(format!("function:user_error: {}: {}", id.0, message))
        }
        FunctionInvocationError::Timeout { timeout_ms, .. } => CamelError::ProcessorError(format!(
            "function:timeout: {} timed out after {}ms",
            id.0, timeout_ms
        )),
        other => CamelError::ProcessorError(format!("function:invoke_error: {}: {other}", id.0)),
    }
}

fn apply_patch(ex: &mut Exchange, patch: ExchangePatch) {
    if let Some(body) = patch.body {
        ex.input.body = match body {
            PatchBody::Text(s) => s.into(),
            PatchBody::Json(v) => v.into(),
            PatchBody::Empty => camel_api::Body::Empty,
        };
    }
    for (k, v) in patch.headers_set {
        ex.input.headers.insert(k, v);
    }
    for k in patch.headers_removed {
        ex.input.headers.remove(&k);
    }
    for (k, v) in patch.properties_set {
        ex.properties.insert(k, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{FunctionInvokerSync, FunctionDiff};
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct MockInvoker {
        responses: Mutex<Vec<Result<ExchangePatch, FunctionInvocationError>>>,
    }

    impl MockInvoker {
        fn new(responses: Vec<Result<ExchangePatch, FunctionInvocationError>>) -> Self {
            Self { responses: Mutex::new(responses) }
        }
    }

    impl FunctionInvokerSync for MockInvoker {
        fn stage_pending(&self, _def: FunctionDefinition, _route_id: Option<&str>, _generation: u64) {}
        fn discard_staging(&self, _generation: u64) {}
        fn begin_reload(&self) -> u64 { 0 }
    }

    #[async_trait]
    impl FunctionInvoker for MockInvoker {
        async fn register(&self, _def: FunctionDefinition, _route_id: Option<&str>) -> Result<(), FunctionInvocationError> { Ok(()) }
        async fn unregister(&self, _id: &FunctionId, _route_id: Option<&str>) -> Result<(), FunctionInvocationError> { Ok(()) }
        async fn invoke(&self, _id: &FunctionId, _exchange: &Exchange) -> Result<ExchangePatch, FunctionInvocationError> {
            let mut resp = self.responses.lock().unwrap();
            resp.remove(0)
        }
        async fn commit_reload(&self, _diff: FunctionDiff, _generation: u64) -> Result<(), FunctionInvocationError> { Ok(()) }
    }

    fn test_definition() -> FunctionDefinition {
        FunctionDefinition {
            id: FunctionId::compute("deno", "test", 5000),
            runtime: "deno".into(),
            source: "test".into(),
            timeout_ms: 5000,
            route_id: None,
            step_index: None,
        }
    }

    #[tokio::test]
    async fn function_step_applies_patch_body_text() {
        let invoker = Arc::new(MockInvoker::new(vec![Ok(ExchangePatch {
            body: Some(PatchBody::Text("patched".into())),
            ..Default::default()
        })]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let mut ex = Exchange::default();
        let result = step.call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("patched"));
    }

    #[tokio::test]
    async fn function_step_applies_patch_headers() {
        let invoker = Arc::new(MockInvoker::new(vec![Ok(ExchangePatch {
            headers_set: vec![("x-key".into(), serde_json::json!("val"))],
            headers_removed: vec!["x-old".into()],
            ..Default::default()
        })]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let mut ex = Exchange::default();
        ex.input.headers.insert("x-old".into(), serde_json::json!("gone"));
        let result = step.call(ex).await.unwrap();
        assert_eq!(result.input.headers.get("x-key").unwrap().as_str(), Some("val"));
        assert!(result.input.headers.get("x-old").is_none());
    }

    #[tokio::test]
    async fn function_step_applies_patch_properties() {
        let invoker = Arc::new(MockInvoker::new(vec![Ok(ExchangePatch {
            properties_set: vec![("prop".into(), serde_json::json!(42))],
            ..Default::default()
        })]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let ex = Exchange::default();
        let result = step.call(ex).await.unwrap();
        assert_eq!(result.properties.get("prop").unwrap().as_i64(), Some(42));
    }

    #[tokio::test]
    async fn function_step_maps_timeout_error() {
        let invoker = Arc::new(MockInvoker::new(vec![Err(FunctionInvocationError::Timeout {
            function_id: FunctionId("x".into()),
            timeout_ms: 5000,
        })]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let ex = Exchange::default();
        let err = step.call(ex).await.unwrap_err();
        let msg = match &err { CamelError::ProcessorError(m) => m, _ => panic!("wrong error type") };
        assert!(msg.contains("function:timeout:"));
    }

    #[tokio::test]
    async fn function_step_maps_user_error() {
        let invoker = Arc::new(MockInvoker::new(vec![Err(FunctionInvocationError::UserError {
            function_id: FunctionId("x".into()),
            message: "boom".into(),
            stack: None,
        })]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let ex = Exchange::default();
        let err = step.call(ex).await.unwrap_err();
        let msg = match &err { CamelError::ProcessorError(m) => m, _ => panic!("wrong error type") };
        assert!(msg.contains("function:user_error:"));
        assert!(msg.contains("boom"));
    }

    #[tokio::test]
    async fn function_step_client_side_timeout_fires() {
        struct SlowInvoker;
        impl FunctionInvokerSync for SlowInvoker {
            fn stage_pending(&self, _def: FunctionDefinition, _route_id: Option<&str>, _generation: u64) {}
            fn discard_staging(&self, _generation: u64) {}
            fn begin_reload(&self) -> u64 { 0 }
        }
        #[async_trait]
        impl FunctionInvoker for SlowInvoker {
            async fn register(&self, _def: FunctionDefinition, _route_id: Option<&str>) -> Result<(), FunctionInvocationError> { Ok(()) }
            async fn unregister(&self, _id: &FunctionId, _route_id: Option<&str>) -> Result<(), FunctionInvocationError> { Ok(()) }
            async fn invoke(&self, _id: &FunctionId, _exchange: &Exchange) -> Result<ExchangePatch, FunctionInvocationError> {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(ExchangePatch::default())
            }
            async fn commit_reload(&self, _diff: FunctionDiff, _generation: u64) -> Result<(), FunctionInvocationError> { Ok(()) }
        }
        let def = FunctionDefinition {
            id: FunctionId::compute("deno", "slow", 50),
            runtime: "deno".into(),
            source: "slow".into(),
            timeout_ms: 50,
            route_id: None,
            step_index: None,
        };
        let mut step = FunctionStep::new(Arc::new(SlowInvoker), def);
        let ex = Exchange::default();
        let err = step.call(ex).await.unwrap_err();
        let msg = match &err { CamelError::ProcessorError(m) => m, _ => panic!("wrong error type") };
        assert!(msg.contains("function:timeout:"));
    }
}
