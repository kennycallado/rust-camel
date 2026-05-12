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
use tracing::Instrument;

#[derive(Clone)]
pub struct FunctionStep {
    definition: FunctionDefinition,
    invoker: Arc<dyn FunctionInvoker>,
}

impl FunctionStep {
    pub fn new(invoker: Arc<dyn FunctionInvoker>, definition: FunctionDefinition) -> Self {
        Self {
            definition,
            invoker,
        }
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
        let runtime = self.definition.runtime.clone();
        let timeout_ms = self.definition.timeout_ms;
        let span = tracing::info_span!(
            target: "camel_function",
            "function",
            function_id = %id.0,
            runtime = %runtime,
            timeout_ms = timeout_ms,
            status = tracing::field::Empty,
            duration_ms = tracing::field::Empty,
            error_kind = tracing::field::Empty,
        );
        Box::pin(async move {
            let start = std::time::Instant::now();
            let outcome: Result<ExchangePatch, CamelError> = async {
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
                Ok(patch)
            }
            .instrument(span.clone())
            .await;
            let elapsed = start.elapsed().as_millis() as u64;
            span.record("duration_ms", elapsed);
            match &outcome {
                Ok(_) => {
                    span.record("status", "ok");
                }
                Err(CamelError::ProcessorError(msg)) => {
                    let kind = if msg.starts_with("function:timeout:") {
                        "timeout"
                    } else if msg.starts_with("function:user_error:") {
                        "user_error"
                    } else if msg.starts_with("function:runner_unavailable:") {
                        "runner_unavailable"
                    } else if msg.starts_with("function:not_registered:") {
                        "not_registered"
                    } else if msg.starts_with("function:transport:") {
                        "transport"
                    } else if msg.starts_with("function:invalid_patch:") {
                        "invalid_patch"
                    } else {
                        "unknown"
                    };
                    span.record("status", kind);
                    span.record("error_kind", kind);
                }
                Err(_) => {
                    span.record("status", "unknown");
                    span.record("error_kind", "unknown");
                }
            }
            let patch = outcome?;
            apply_patch(&mut ex, patch);
            Ok(ex)
        })
    }
}

fn map_invocation_error(err: FunctionInvocationError, id: &FunctionId) -> CamelError {
    match err {
        FunctionInvocationError::UserError { message, stack, .. } => {
            let detail = match stack {
                Some(s) if !s.is_empty() => {
                    format!("function:user_error: {}: {}\n{}", id.0, message, s)
                }
                _ => format!("function:user_error: {}: {}", id.0, message),
            };
            CamelError::ProcessorError(detail)
        }
        FunctionInvocationError::Timeout { timeout_ms, .. } => CamelError::ProcessorError(format!(
            "function:timeout: {} timed out after {}ms",
            id.0, timeout_ms
        )),
        FunctionInvocationError::NotRegistered { .. } => {
            CamelError::ProcessorError(format!("function:not_registered: {}", id.0))
        }
        FunctionInvocationError::RunnerUnavailable { reason } => {
            CamelError::ProcessorError(format!("function:runner_unavailable: {}: {}", id.0, reason))
        }
        FunctionInvocationError::Transport(msg) => {
            CamelError::ProcessorError(format!("function:transport: {}: {}", id.0, msg))
        }
        FunctionInvocationError::InvalidPatch(msg) => {
            CamelError::ProcessorError(format!("function:invalid_patch: {}: {}", id.0, msg))
        }
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
    use async_trait::async_trait;
    use camel_api::function::PrepareToken;
    use camel_api::{FunctionDiff, FunctionInvokerSync};
    use std::sync::Mutex;

    struct MockInvoker {
        responses: Mutex<Vec<Result<ExchangePatch, FunctionInvocationError>>>,
    }

    impl MockInvoker {
        fn new(responses: Vec<Result<ExchangePatch, FunctionInvocationError>>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    impl FunctionInvokerSync for MockInvoker {
        fn stage_pending(
            &self,
            _def: FunctionDefinition,
            _route_id: Option<&str>,
            _generation: u64,
        ) {
        }
        fn discard_staging(&self, _generation: u64) {}
        fn begin_reload(&self) -> u64 {
            0
        }
        fn function_refs_for_route(&self, _route_id: &str) -> Vec<(FunctionId, Option<String>)> {
            vec![]
        }
        fn staged_refs_for_route(
            &self,
            _route_id: &str,
            _generation: u64,
        ) -> Vec<(FunctionId, Option<String>)> {
            vec![]
        }
        fn staged_defs_for_route(
            &self,
            _route_id: &str,
            _generation: u64,
        ) -> Vec<(FunctionDefinition, Option<String>)> {
            vec![]
        }
    }

    #[async_trait]
    impl FunctionInvoker for MockInvoker {
        async fn register(
            &self,
            _def: FunctionDefinition,
            _route_id: Option<&str>,
        ) -> Result<(), FunctionInvocationError> {
            Ok(())
        }
        async fn unregister(
            &self,
            _id: &FunctionId,
            _route_id: Option<&str>,
        ) -> Result<(), FunctionInvocationError> {
            Ok(())
        }
        async fn invoke(
            &self,
            _id: &FunctionId,
            _exchange: &Exchange,
        ) -> Result<ExchangePatch, FunctionInvocationError> {
            let mut resp = self.responses.lock().unwrap();
            resp.remove(0)
        }
        async fn prepare_reload(
            &self,
            _diff: FunctionDiff,
            _generation: u64,
        ) -> Result<PrepareToken, FunctionInvocationError> {
            Ok(PrepareToken::default())
        }
        async fn finalize_reload(
            &self,
            _diff: &FunctionDiff,
            _generation: u64,
        ) -> Result<(), FunctionInvocationError> {
            Ok(())
        }
        async fn rollback_reload(
            &self,
            _token: PrepareToken,
            _generation: u64,
        ) -> Result<(), FunctionInvocationError> {
            Ok(())
        }
        async fn commit_reload(
            &self,
            _diff: FunctionDiff,
            _generation: u64,
        ) -> Result<(), FunctionInvocationError> {
            Ok(())
        }
        async fn commit_staged(&self) -> Result<(), FunctionInvocationError> {
            Ok(())
        }
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
        let ex = Exchange::default();
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
        ex.input
            .headers
            .insert("x-old".into(), serde_json::json!("gone"));
        let result = step.call(ex).await.unwrap();
        assert_eq!(
            result.input.headers.get("x-key").unwrap().as_str(),
            Some("val")
        );
        assert!(!result.input.headers.contains_key("x-old"));
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
        let invoker = Arc::new(MockInvoker::new(vec![Err(
            FunctionInvocationError::Timeout {
                function_id: FunctionId("x".into()),
                timeout_ms: 5000,
            },
        )]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let ex = Exchange::default();
        let err = step.call(ex).await.unwrap_err();
        let msg = match &err {
            CamelError::ProcessorError(m) => m,
            _ => panic!("wrong error type"),
        };
        assert!(msg.contains("function:timeout:"));
    }

    #[tokio::test]
    async fn function_step_maps_user_error() {
        let invoker = Arc::new(MockInvoker::new(vec![Err(
            FunctionInvocationError::UserError {
                function_id: FunctionId("x".into()),
                message: "boom".into(),
                stack: None,
            },
        )]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let ex = Exchange::default();
        let err = step.call(ex).await.unwrap_err();
        let msg = match &err {
            CamelError::ProcessorError(m) => m,
            _ => panic!("wrong error type"),
        };
        assert!(msg.contains("function:user_error:"));
        assert!(msg.contains("boom"));
    }

    #[tokio::test]
    async fn function_step_client_side_timeout_fires() {
        struct SlowInvoker;
        impl FunctionInvokerSync for SlowInvoker {
            fn stage_pending(
                &self,
                _def: FunctionDefinition,
                _route_id: Option<&str>,
                _generation: u64,
            ) {
            }
            fn discard_staging(&self, _generation: u64) {}
            fn begin_reload(&self) -> u64 {
                0
            }
            fn function_refs_for_route(
                &self,
                _route_id: &str,
            ) -> Vec<(FunctionId, Option<String>)> {
                vec![]
            }
            fn staged_refs_for_route(
                &self,
                _route_id: &str,
                _generation: u64,
            ) -> Vec<(FunctionId, Option<String>)> {
                vec![]
            }
            fn staged_defs_for_route(
                &self,
                _route_id: &str,
                _generation: u64,
            ) -> Vec<(FunctionDefinition, Option<String>)> {
                vec![]
            }
        }
        #[async_trait]
        impl FunctionInvoker for SlowInvoker {
            async fn register(
                &self,
                _def: FunctionDefinition,
                _route_id: Option<&str>,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn unregister(
                &self,
                _id: &FunctionId,
                _route_id: Option<&str>,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn invoke(
                &self,
                _id: &FunctionId,
                _exchange: &Exchange,
            ) -> Result<ExchangePatch, FunctionInvocationError> {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(ExchangePatch::default())
            }
            async fn prepare_reload(
                &self,
                _diff: FunctionDiff,
                _generation: u64,
            ) -> Result<PrepareToken, FunctionInvocationError> {
                Ok(PrepareToken::default())
            }
            async fn finalize_reload(
                &self,
                _diff: &FunctionDiff,
                _generation: u64,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn rollback_reload(
                &self,
                _token: PrepareToken,
                _generation: u64,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn commit_reload(
                &self,
                _diff: FunctionDiff,
                _generation: u64,
            ) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
            async fn commit_staged(&self) -> Result<(), FunctionInvocationError> {
                Ok(())
            }
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
        let msg = match &err {
            CamelError::ProcessorError(m) => m,
            _ => panic!("wrong error type"),
        };
        assert!(msg.contains("function:timeout:"));
    }

    #[tokio::test]
    async fn function_step_emits_tracing_span() {
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
        use tracing_subscriber::prelude::*;

        let invoker = Arc::new(MockInvoker::new(vec![Ok(ExchangePatch::default())]));
        let def = FunctionDefinition {
            id: FunctionId::compute("deno", "span_test", 5000),
            runtime: "deno".into(),
            source: "span_test".into(),
            timeout_ms: 5000,
            route_id: None,
            step_index: None,
        };
        let mut step = FunctionStep::new(invoker, def);
        let ex = Exchange::default();

        let span_seen = Arc::new(AtomicBool::new(false));
        let span_seen_clone = span_seen.clone();

        let layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::sink)
            .with_filter(tracing_subscriber::filter::filter_fn(move |meta| {
                if meta.target() == "camel_function" && meta.name() == "function" {
                    span_seen_clone.store(true, AtomicOrdering::SeqCst);
                }
                true
            }));

        let _guard = tracing_subscriber::registry().with(layer).set_default();
        let result = step.call(ex).await;
        assert!(result.is_ok());
        assert!(
            span_seen.load(AtomicOrdering::SeqCst),
            "expected function span with target 'camel_function' and name 'function'"
        );
    }

    #[tokio::test]
    async fn function_step_user_error_with_stack() {
        let invoker = Arc::new(MockInvoker::new(vec![Err(
            FunctionInvocationError::UserError {
                function_id: FunctionId("x".into()),
                message: "custom error".into(),
                stack: Some("at line 1\nat line 2".into()),
            },
        )]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let ex = Exchange::default();
        let err = step.call(ex).await.unwrap_err();
        let msg = match &err {
            CamelError::ProcessorError(m) => m.clone(),
            _ => panic!("wrong error type"),
        };
        assert!(msg.contains("function:user_error:"));
        assert!(msg.contains("custom error"));
        assert!(msg.contains("at line 1"));
    }

    #[tokio::test]
    async fn function_step_maps_not_registered_error() {
        let invoker = Arc::new(MockInvoker::new(vec![Err(
            FunctionInvocationError::NotRegistered {
                function_id: FunctionId("missing-fn".into()),
            },
        )]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let ex = Exchange::default();
        let err = step.call(ex).await.unwrap_err();
        let msg = match &err {
            CamelError::ProcessorError(m) => m,
            other => panic!("wrong error type: {:?}", other),
        };
        assert!(msg.contains("function:not_registered:"), "msg was: {}", msg);
    }

    #[tokio::test]
    async fn function_step_maps_runner_unavailable_error() {
        let invoker = Arc::new(MockInvoker::new(vec![Err(
            FunctionInvocationError::RunnerUnavailable {
                reason: "runtime crashed".into(),
            },
        )]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let ex = Exchange::default();
        let err = step.call(ex).await.unwrap_err();
        let msg = match &err {
            CamelError::ProcessorError(m) => m,
            _ => panic!("wrong error type"),
        };
        assert!(msg.contains("function:runner_unavailable:"));
        assert!(msg.contains("runtime crashed"));
    }

    #[tokio::test]
    async fn function_step_maps_transport_error() {
        let invoker = Arc::new(MockInvoker::new(vec![Err(
            FunctionInvocationError::Transport("connection refused".into()),
        )]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let ex = Exchange::default();
        let err = step.call(ex).await.unwrap_err();
        let msg = match &err {
            CamelError::ProcessorError(m) => m,
            _ => panic!("wrong error type"),
        };
        assert!(msg.contains("function:transport:"));
        assert!(msg.contains("connection refused"));
    }

    #[tokio::test]
    async fn function_step_maps_invalid_patch_error() {
        let invoker = Arc::new(MockInvoker::new(vec![Err(
            FunctionInvocationError::InvalidPatch("missing field 'body'".into()),
        )]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let ex = Exchange::default();
        let err = step.call(ex).await.unwrap_err();
        let msg = match &err {
            CamelError::ProcessorError(m) => m,
            _ => panic!("wrong error type"),
        };
        assert!(msg.contains("function:invalid_patch:"));
        assert!(msg.contains("missing field 'body'"));
    }

    #[tokio::test]
    async fn function_step_applies_patch_body_json() {
        let invoker = Arc::new(MockInvoker::new(vec![Ok(ExchangePatch {
            body: Some(PatchBody::Json(serde_json::json!({"key": "value"}))),
            ..Default::default()
        })]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let ex = Exchange::default();
        let result = step.call(ex).await.unwrap();
        match result.input.body {
            camel_api::Body::Json(v) => assert_eq!(v.get("key").unwrap().as_str(), Some("value")),
            other => panic!("expected Json body, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn function_step_applies_patch_body_empty() {
        let invoker = Arc::new(MockInvoker::new(vec![Ok(ExchangePatch {
            body: Some(PatchBody::Empty),
            ..Default::default()
        })]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let ex = Exchange::default();
        let result = step.call(ex).await.unwrap();
        assert!(matches!(result.input.body, camel_api::Body::Empty));
    }

    #[tokio::test]
    async fn function_step_poll_ready_always_ready() {
        let invoker = Arc::new(MockInvoker::new(vec![Ok(ExchangePatch::default())]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let mut cx = std::task::Context::from_waker(futures::task::noop_waker_ref());
        let poll = step.poll_ready(&mut cx);
        assert!(matches!(poll, std::task::Poll::Ready(Ok(()))));
    }

    #[tokio::test]
    async fn function_step_user_error_with_empty_stack() {
        let invoker = Arc::new(MockInvoker::new(vec![Err(
            FunctionInvocationError::UserError {
                function_id: FunctionId("x".into()),
                message: "no stack".into(),
                stack: Some("".into()),
            },
        )]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let ex = Exchange::default();
        let err = step.call(ex).await.unwrap_err();
        let msg = match &err {
            CamelError::ProcessorError(m) => m,
            _ => panic!("wrong error type"),
        };
        assert!(msg.contains("function:user_error:"));
        assert!(msg.contains("no stack"));
        assert!(!msg.contains("\n"));
    }

    #[tokio::test]
    async fn function_step_preserves_exchange_properties_not_in_patch() {
        let invoker = Arc::new(MockInvoker::new(vec![Ok(ExchangePatch {
            body: Some(PatchBody::Text("new body".into())),
            ..Default::default()
        })]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let mut ex = Exchange::default();
        ex.properties
            .insert("existing".into(), serde_json::json!("keep"));
        let result = step.call(ex).await.unwrap();
        assert_eq!(
            result.properties.get("existing").unwrap().as_str(),
            Some("keep")
        );
        assert_eq!(result.input.body.as_text(), Some("new body"));
    }

    #[tokio::test]
    async fn function_step_removes_header_not_in_set() {
        let invoker = Arc::new(MockInvoker::new(vec![Ok(ExchangePatch {
            headers_removed: vec!["to-remove".into()],
            ..Default::default()
        })]));
        let mut step = FunctionStep::new(invoker, test_definition());
        let mut ex = Exchange::default();
        ex.input
            .headers
            .insert("to-remove".into(), serde_json::json!("old"));
        ex.input
            .headers
            .insert("to-keep".into(), serde_json::json!("stay"));
        let result = step.call(ex).await.unwrap();
        assert!(!result.input.headers.contains_key("to-remove"));
        assert!(result.input.headers.contains_key("to-keep"));
    }
}
