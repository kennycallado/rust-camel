use async_trait::async_trait;
use camel_api::function::*;
use camel_api::lifecycle::Lifecycle;
use std::sync::Arc;

#[test]
fn function_id_compute_is_deterministic() {
    let a = FunctionId::compute("deno", "return 1", 5000);
    let b = FunctionId::compute("deno", "return 1", 5000);
    assert_eq!(a, b);
}

#[test]
fn function_id_varies_on_inputs() {
    let a = FunctionId::compute("deno", "return 1", 5000);
    let b = FunctionId::compute("deno", "return 2", 5000);
    let c = FunctionId::compute("deno", "return 1", 10000);
    assert_ne!(a, b);
    assert_ne!(a, c);
}

#[test]
fn function_id_hex_format() {
    let id = FunctionId::compute("deno", "test", 5000);
    assert_eq!(id.0.len(), 32);
    assert!(id.0.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(!id.0.chars().any(|c| c.is_ascii_uppercase()));
}

#[test]
fn function_definition_clone_debug() {
    let def = FunctionDefinition {
        id: FunctionId::compute("deno", "test", 5000),
        runtime: "deno".into(),
        source: "test".into(),
        timeout_ms: 5000,
        route_id: Some("route-1".into()),
        step_index: Some(0),
    };
    let cloned = def.clone();
    assert_eq!(def.id, cloned.id);
    let _ = format!("{:?}", def);
}

#[test]
fn invocation_error_display() {
    let id = FunctionId::compute("deno", "test", 5000);
    let e = FunctionInvocationError::NotRegistered {
        function_id: id.clone(),
    };
    let msg = e.to_string();
    assert!(msg.contains("not registered"));

    let e = FunctionInvocationError::Timeout {
        function_id: id.clone(),
        timeout_ms: 5000,
    };
    assert!(e.to_string().contains("timed out"));

    let e = FunctionInvocationError::RunnerUnavailable {
        reason: "boom".into(),
    };
    assert!(e.to_string().contains("runner unavailable"));

    let e = FunctionInvocationError::UserError {
        function_id: id.clone(),
        message: "fail".into(),
        stack: Some("at line 1".into()),
    };
    assert!(e.to_string().contains("user code failed"));

    let e = FunctionInvocationError::Transport("net err".into());
    assert!(e.to_string().contains("transport"));

    let e = FunctionInvocationError::InvalidPatch("bad".into());
    assert!(e.to_string().contains("invalid patch"));
}

#[test]
fn invocation_error_is_std_error() {
    fn assert_error<E: std::error::Error>(_: &E) {}
    let id = FunctionId::compute("deno", "test", 5000);
    assert_error(&FunctionInvocationError::NotRegistered { function_id: id });
    assert_error(&FunctionInvocationError::Timeout {
        function_id: FunctionId("x".into()),
        timeout_ms: 100,
    });
    assert_error(&FunctionInvocationError::RunnerUnavailable { reason: "x".into() });
    assert_error(&FunctionInvocationError::UserError {
        function_id: FunctionId("x".into()),
        message: "x".into(),
        stack: None,
    });
    assert_error(&FunctionInvocationError::Transport("x".into()));
    assert_error(&FunctionInvocationError::InvalidPatch("x".into()));
}

#[test]
fn lifecycle_default_returns_none_for_function_invoker() {
    struct DummyService;
    #[async_trait]
    impl Lifecycle for DummyService {
        fn name(&self) -> &str {
            "dummy"
        }
        async fn start(&mut self) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
        async fn stop(&mut self) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
    }
    let svc = DummyService;
    assert!(svc.as_function_invoker().is_none());
}

#[test]
fn lifecycle_impl_returns_some_for_function_invoker() {
    struct InvokerService;
    impl FunctionInvokerSync for InvokerService {
        fn stage_pending(&self, _def: FunctionDefinition, _route_id: Option<&str>, _gen: u64) {}
        fn discard_staging(&self, _gen: u64) {}
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
    impl FunctionInvoker for InvokerService {
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
            _exchange: &camel_api::Exchange,
        ) -> Result<ExchangePatch, FunctionInvocationError> {
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
        async fn commit_staged(&self) -> Result<(), FunctionInvocationError> {
            Ok(())
        }
    }
    #[async_trait]
    impl Lifecycle for InvokerService {
        fn name(&self) -> &str {
            "invoker-svc"
        }
        async fn start(&mut self) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
        async fn stop(&mut self) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
        fn as_function_invoker(&self) -> Option<Arc<dyn FunctionInvoker>> {
            Some(Arc::new(InvokerService))
        }
    }
    let svc = InvokerService;
    assert!(svc.as_function_invoker().is_some());
}

#[test]
fn exchange_patch_default_is_empty() {
    let patch = ExchangePatch::default();
    assert!(patch.body.is_none());
    assert!(patch.headers_set.is_empty());
    assert!(patch.headers_removed.is_empty());
    assert!(patch.properties_set.is_empty());
}

#[test]
fn function_invoker_is_object_safe() {
    struct MockInvoker;
    impl FunctionInvokerSync for MockInvoker {
        fn stage_pending(&self, _def: FunctionDefinition, _route_id: Option<&str>, _gen: u64) {}
        fn discard_staging(&self, _gen: u64) {}
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
            _exchange: &camel_api::Exchange,
        ) -> Result<ExchangePatch, FunctionInvocationError> {
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
        async fn commit_staged(&self) -> Result<(), FunctionInvocationError> {
            Ok(())
        }
    }
    let _: Arc<dyn FunctionInvoker> = Arc::new(MockInvoker);
}
