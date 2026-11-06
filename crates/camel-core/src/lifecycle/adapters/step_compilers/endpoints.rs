//! Endpoint step compilers: To, WireTap.
//!
//! These steps create producers from URIs at compile time.

use camel_api::SpanKindHint;
use std::sync::Arc;

use camel_api::{BoxProcessor, CamelError, LayerFactory, StepLifecycle};

use super::{
    CompilationContext, CompileOutcome, CompiledStep, StepCompiler, StepCompilerRegistry,
    resolve_producer_with_lifecycle, resolve_send,
};
use crate::intercept::InterceptAction;
use crate::lifecycle::adapters::CompositeStepLifecycle;
use crate::lifecycle::application::route_definition::BuilderStep;

pub(crate) struct EndpointsCompiler;

/// Enrich a failed intercept-target resolution (Tasks 4+5): the
/// `ComponentNotFound` variant is preserved (operators match on it to
/// distinguish "component not registered" from configuration errors); other
/// variants are rewrapped as `Config`.
fn enrich_intercept_error(err: CamelError, target: &str) -> CamelError {
    match err {
        CamelError::ComponentNotFound(s) => {
            CamelError::ComponentNotFound(format!("{s} (intercept target: {target})"))
        }
        other => CamelError::Config(format!("{other} (intercept target: {target})")),
    }
}

/// Compile an un-diverted (optionally substituted) `To` send.
fn compile_to(
    ctx: &CompilationContext,
    uri: &str,
    intercept_target: Option<&str>,
) -> Result<CompileOutcome, CamelError> {
    match (resolve_send(ctx, uri), intercept_target) {
        (Err(e), Some(target)) => Err(enrich_intercept_error(e, target)),
        (Err(e), None) => Err(e),
        (Ok(resolved), _) => Ok(CompileOutcome::Matched(CompiledStep::Process {
            kind_hint: SpanKindHint::Internal,
            processor: resolved.producer,
            body_contract: resolved.body_contract,
            lifecycle: resolved.lifecycle,
            label: None,
            to_uri: None,
        })),
    }
}

/// Compile a middleware-wrapped send (`LayeredTo`).
///
/// Resolution is the SAME path as [`compile_to`] (`resolve_send`), including
/// the intercept-target error enrichment, so a `SkipTo`-substituted
/// `LayeredTo` reports component/config failures exactly like an interrupted
/// `To`. The only difference is the final wrap: the factory's layer wraps the
/// resolved producer. A wrap failure aborts route-add with the layer's
/// message. `to_uri` stays `None` here: the registry stamps the authored send
/// URI centrally from `BuilderStep::to_uri_metadata`.
fn compile_to_layered(
    ctx: &CompilationContext,
    uri: &str,
    factory: &LayerFactory,
    intercept_target: Option<&str>,
) -> Result<CompileOutcome, CamelError> {
    let resolved = match (resolve_send(ctx, uri), intercept_target) {
        (Err(e), Some(target)) => return Err(enrich_intercept_error(e, target)),
        (Err(e), None) => return Err(e),
        (Ok(resolved), _) => resolved,
    };
    let processor = factory.wrap(resolved.producer)?;
    Ok(CompileOutcome::Matched(CompiledStep::Process {
        kind_hint: SpanKindHint::Internal,
        processor,
        body_contract: resolved.body_contract,
        lifecycle: resolved.lifecycle,
        label: None,
        to_uri: None,
    }))
}

/// Compile a diverted send (Task 5): a wiretap copy stage in front of the
/// real producer.
///
/// The copy stage is infallible at the pipeline level — copy readiness and
/// call failures are logged and suppressed inside the wiretap — so the real
/// producer's `Result` is returned verbatim to the caller. The body contract
/// comes from the REAL endpoint: the copy never replaces the main exchange.
///
/// Lifecycle: children in compile order `[copy endpoint?, tap, real
/// endpoint?]`. `CompositeStepLifecycle::shutdown` iterates in REVERSE — the
/// real endpoint tears down first, the tap then drains in-flight copies, and
/// the copy endpoint closes last (copies drain before their target endpoint
/// tears down). The child list is built ONCE per compile because
/// `shutdown_called` is a per-handle flag: every start/shutdown of this step
/// must flow through the same composite chain.
fn compile_divert(
    ctx: &CompilationContext,
    real_uri: &str,
    copy_uri: &str,
) -> Result<CompileOutcome, CamelError> {
    // Real side: the un-intercepted resolution path, verbatim.
    let real = resolve_send(ctx, real_uri)?;
    // Copy side: producer (+ endpoint lifecycle) from the copy target.
    let (copy_producer, copy_lifecycle) = resolve_producer_with_lifecycle(ctx, copy_uri)
        .map_err(|e| enrich_intercept_error(e, copy_uri))?;

    // The tap clone moved into the composition shares the admission gate and
    // task tracker with the lifecycle handle, so route shutdown drains the
    // same tracker the processor admits into.
    let tap = camel_processor::WireTapService::new(copy_producer);
    let processor = camel_processor::compose_divert(tap.clone(), real.producer);

    let mut children: Vec<Arc<dyn StepLifecycle>> = Vec::with_capacity(3);
    if let Some(lc) = copy_lifecycle {
        children.push(lc);
    }
    children.push(tap.lifecycle());
    if let Some(lc) = real.lifecycle {
        children.push(lc);
    }
    let lifecycle = Some(Arc::new(CompositeStepLifecycle::new(children)) as Arc<dyn StepLifecycle>);

    Ok(CompileOutcome::Matched(CompiledStep::Process {
        kind_hint: SpanKindHint::Internal,
        processor,
        body_contract: real.body_contract,
        lifecycle,
        label: None,
        to_uri: None,
    }))
}

impl StepCompiler for EndpointsCompiler {
    fn compile(
        &self,
        step: BuilderStep,
        _step_index: usize,
        ctx: &CompilationContext,
        _registry: &StepCompilerRegistry,
    ) -> Result<CompileOutcome, CamelError> {
        match step {
            // ── To ──
            BuilderStep::To(uri) => {
                // Route send-point interception: a `SkipTo` rule (Task 4)
                // substitutes the send URI before any parsing or component
                // resolution, so the original URI's scheme is never resolved.
                // A `DivertCopyTo` rule (Task 5) composes a copy stage in
                // front of the real producer.
                match ctx.intercept.lookup(&uri) {
                    None => compile_to(ctx, &uri, None),
                    Some(InterceptAction::SkipTo { uri: target }) => {
                        compile_to(ctx, target, Some(target.as_str()))
                    }
                    Some(InterceptAction::DivertCopyTo { uri: copy_uri }) => {
                        compile_divert(ctx, &uri, copy_uri)
                    }
                }
            }

            // ── LayeredTo ──
            //
            // Intercept lookup is identical to `To` (same rules, same order):
            // `SkipTo` substitutes the URI before resolution and `LayeredTo`
            // keeps its wrap; `DivertCopyTo` is intentionally NOT supported.
            // A diverted send needs the composite copy+real pipeline
            // (`compile_divert`); silently downgrading it to a `SkipTo`-style
            // substitution would suppress the copy delivery. The blessed spec
            // has no divert+layer scenario, so this is an explicit narrowing
            // of design.md's "interception aware" sentence: loud failure over
            // silently wrong semantics. Plain `To` under `DivertCopyTo` still
            // goes through `compile_divert` unchanged.
            BuilderStep::LayeredTo { uri, factory } => match ctx.intercept.lookup(&uri) {
                None => compile_to_layered(ctx, &uri, &factory, None),
                Some(InterceptAction::SkipTo { uri: target }) => {
                    compile_to_layered(ctx, target, &factory, Some(target.as_str()))
                }
                Some(InterceptAction::DivertCopyTo { .. }) => Err(CamelError::RouteError(
                    "to_layered does not support DivertCopyTo interception; use .to() or a SkipTo rule for the intercepted URI".into(),
                )),
            },

            // ── WireTap ──
            BuilderStep::WireTap { uri } => {
                // WireTap needs the same lifecycle capture as `To`. The
                // shared `resolve_producer` helper does not surface the
                // endpoint, so use the parallel
                // `resolve_producer_with_lifecycle` helper that returns
                // `(BoxProcessor, Option<Arc<dyn StepLifecycle>>)`.
                let (producer, endpoint_lifecycle) = resolve_producer_with_lifecycle(ctx, &uri)?;
                let svc = camel_processor::WireTapService::new(producer);
                let wiretap_lifecycle: Arc<dyn StepLifecycle> = svc.lifecycle();
                // Compose endpoint + WireTap lifecycles so shutdown drains
                // WireTap before tearing down the endpoint (reverse order).
                let lifecycle = match endpoint_lifecycle {
                    Some(ep) => {
                        Some(
                            Arc::new(CompositeStepLifecycle::new(vec![ep, wiretap_lifecycle]))
                                as Arc<dyn StepLifecycle>,
                        )
                    }
                    None => Some(wiretap_lifecycle),
                };
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    kind_hint: SpanKindHint::Internal,
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle,
                    label: None,
                    to_uri: None,
                }))
            }

            _ => Ok(CompileOutcome::NotHandled(step)),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the Endpoint::lifecycle() wiring into `CompiledStep::Process`.
    //!
    //! These tests stand up a minimal `Component` + `Endpoint` pair whose
    //! endpoint overrides `lifecycle()` to surface a `StepLifecycle` handle.
    //! The `To` and `WireTap` arms of `EndpointsCompiler` must propagate that
    //! handle into the produced `CompiledStep::Process` so the route
    //! controller can start/shut it down in route order (ADR-0022).

    use super::*;
    use crate::ClaimCheckRegistry;
    use crate::IdempotentRegistry;
    use crate::lifecycle::adapters::step_resolution::FunctionStagingMode;
    use async_trait::async_trait;
    use camel_api::{
        BodyType, BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message, StepLifecycle,
        StepShutdownReason,
    };
    use camel_bean::BeanRegistry;
    use camel_component_api::{
        Component, ComponentContext, Endpoint, ProducerContext, RuntimeObservability,
        test_support::NoopRuntimeObservability,
    };
    use camel_endpoint::parse_uri;
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};
    use tower::Service;
    use tower::ServiceExt;

    /// Minimal `StepLifecycle` for tests. Exists here (not in camel-api)
    /// because `camel-component-api` must not depend on test types from
    /// the deeper crate, and these tests are in `camel-core` (which CAN
    /// see both `camel-api::StepLifecycle` and
    /// `camel-component_api::Endpoint`).
    #[derive(Debug)]
    struct FakeStep;

    #[async_trait]
    impl StepLifecycle for FakeStep {
        fn name(&self) -> &'static str {
            "fake"
        }
        async fn shutdown(&self, _reason: StepShutdownReason) -> Result<(), CamelError> {
            Ok(())
        }
    }

    /// Endpoint that exposes a `StepLifecycle` via `lifecycle()`. Other
    /// methods are no-op shims (the tests never call them).
    struct StatefulEndpoint {
        uri: String,
        handle: Arc<dyn StepLifecycle>,
    }

    impl Endpoint for StatefulEndpoint {
        fn uri(&self) -> &str {
            &self.uri
        }

        fn create_consumer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
        ) -> Result<Box<dyn camel_component_api::Consumer>, CamelError> {
            Err(CamelError::EndpointCreationFailed("not a consumer".into()))
        }

        fn create_producer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
            _ctx: &ProducerContext,
        ) -> Result<BoxProcessor, CamelError> {
            // Identity processor: returns the exchange unchanged. Sufficient
            // for testing that the `To`/`WireTap` arms propagate the
            // lifecycle handle; the produced value is never invoked.
            Ok(BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) })))
        }

        fn lifecycle(&self) -> Option<Arc<dyn StepLifecycle>> {
            Some(self.handle.clone())
        }
    }

    /// Component that vends `StatefulEndpoint` for the `stateful:` scheme.
    struct StatefulComponent {
        handle: Arc<dyn StepLifecycle>,
    }

    #[async_trait]
    impl Component for StatefulComponent {
        fn scheme(&self) -> &str {
            "stateful"
        }

        fn create_endpoint(
            &self,
            uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(StatefulEndpoint {
                uri: uri.to_string(),
                handle: self.handle.clone(),
            }))
        }
    }

    /// Parameterized `ComponentContext` shared by every test in this module.
    ///
    /// Replaces the four copy-pasted per-scheme contexts (`StatefulContext`,
    /// `StatelessContext`, `ProbeContext`, `NeverReadyContext`) with a single
    /// scheme→component map, so the `ComponentContext` boilerplate (languages,
    /// metrics, platform service, health checks) lives in ONE place.
    struct TestCompilerContext {
        components: HashMap<String, Arc<dyn Component>>,
    }

    impl TestCompilerContext {
        fn new() -> Self {
            Self {
                components: HashMap::new(),
            }
        }

        fn with(mut self, component: Arc<dyn Component>) -> Self {
            self.components
                .insert(component.scheme().to_string(), component);
            self
        }
    }

    impl ComponentContext for TestCompilerContext {
        fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>> {
            self.components.get(scheme).cloned()
        }
        fn resolve_language(&self, _name: &str) -> Option<Arc<dyn camel_language_api::Language>> {
            None
        }
        fn metrics(&self) -> Arc<dyn camel_api::MetricsCollector> {
            Arc::new(camel_api::NoOpMetrics)
        }
        fn platform_service(&self) -> Arc<dyn camel_api::PlatformService> {
            Arc::new(camel_api::NoopPlatformService::default())
        }
        fn register_route_health_check(
            &self,
            _route_id: &str,
            _check: Arc<dyn camel_api::AsyncHealthCheck>,
        ) {
        }
        fn unregister_route_health_check(&self, _route_id: &str) {}
    }

    /// Context resolving `stateful:` to an endpoint that exposes `handle`.
    fn stateful_context(handle: Arc<dyn StepLifecycle>) -> Arc<dyn ComponentContext> {
        Arc::new(TestCompilerContext::new().with(Arc::new(StatefulComponent { handle })))
    }

    /// Context resolving `stateless:` to an endpoint with no lifecycle handle.
    fn stateless_context() -> Arc<dyn ComponentContext> {
        Arc::new(TestCompilerContext::new().with(Arc::new(StatelessComponent)))
    }

    /// Context resolving `test:` to the in-flight probe endpoint.
    fn probe_context(
        inflight: Arc<AtomicUsize>,
        max_inflight: Arc<AtomicUsize>,
    ) -> Arc<dyn ComponentContext> {
        Arc::new(TestCompilerContext::new().with(Arc::new(ProbeComponent {
            inflight,
            max_inflight,
        })))
    }

    /// Context resolving `neverready:` to a producer whose `poll_ready` fails.
    fn never_ready_context() -> Arc<dyn ComponentContext> {
        Arc::new(TestCompilerContext::new().with(Arc::new(NeverReadyComponent)))
    }

    /// `StepLifecycle` fake that tracks how many times `shutdown` was called.
    #[derive(Debug)]
    struct ShutdownTrackingFake {
        count: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl StepLifecycle for ShutdownTrackingFake {
        fn name(&self) -> &'static str {
            "shutdown-tracking-fake"
        }
        async fn shutdown(&self, _reason: StepShutdownReason) -> Result<(), CamelError> {
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    /// Lifecycle fake that records its id on `shutdown` (Task 5: divert
    /// child ordering).
    #[derive(Debug)]
    struct RecordingShutdown {
        id: &'static str,
        order: Arc<Mutex<Vec<&'static str>>>,
    }

    impl RecordingShutdown {
        fn new(id: &'static str, order: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self { id, order }
        }
    }

    #[async_trait]
    impl StepLifecycle for RecordingShutdown {
        fn name(&self) -> &'static str {
            self.id
        }
        async fn shutdown(&self, _reason: StepShutdownReason) -> Result<(), CamelError> {
            self.order.lock().expect("order mutex").push(self.id);
            Ok(())
        }
    }

    /// Divert composition order (Task 5): children in compile order
    /// `[copy, tap, real]`; shutdown iterates in REVERSE — the real endpoint
    /// tears down first, the tap then drains in-flight copies, and the copy
    /// endpoint closes last (`composite_step_lifecycle.rs:66-67`).
    #[tokio::test]
    async fn divert_children_shut_down_in_reverse_order() {
        let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let copy: Arc<dyn StepLifecycle> =
            Arc::new(RecordingShutdown::new("copy", Arc::clone(&order)));
        let tap: Arc<dyn StepLifecycle> =
            Arc::new(RecordingShutdown::new("tap", Arc::clone(&order)));
        let real: Arc<dyn StepLifecycle> =
            Arc::new(RecordingShutdown::new("real", Arc::clone(&order)));

        let composite = CompositeStepLifecycle::new(vec![copy, tap, real]);
        composite
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .expect("shutdown should succeed");

        let recorded = order.lock().expect("order mutex").clone();
        assert_eq!(
            recorded,
            vec!["real", "tap", "copy"],
            "shutdown must run children in reverse: real, tap drain, copy close"
        );
    }

    /// Endpoint that returns `None` from `lifecycle()` — stateless.
    struct StatelessEndpoint {
        uri: String,
    }

    impl Endpoint for StatelessEndpoint {
        fn uri(&self) -> &str {
            &self.uri
        }

        fn create_consumer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
        ) -> Result<Box<dyn camel_component_api::Consumer>, CamelError> {
            Err(CamelError::EndpointCreationFailed("not a consumer".into()))
        }

        fn create_producer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
            _ctx: &ProducerContext,
        ) -> Result<BoxProcessor, CamelError> {
            Ok(BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) })))
        }

        fn lifecycle(&self) -> Option<Arc<dyn StepLifecycle>> {
            None
        }
    }

    /// Component that vends `StatelessEndpoint` for the `stateless:` scheme.
    struct StatelessComponent;

    #[async_trait]
    impl Component for StatelessComponent {
        fn scheme(&self) -> &str {
            "stateless"
        }

        fn create_endpoint(
            &self,
            uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(StatelessEndpoint {
                uri: uri.to_string(),
            }))
        }
    }

    /// Endpoint whose producer tracks concurrent in-flight calls and exposes a
    /// body contract, so `LayeredTo` can be compared against plain `To` on
    /// observable compile output.
    struct ProbeEndpoint {
        uri: String,
        inflight: Arc<AtomicUsize>,
        max_inflight: Arc<AtomicUsize>,
    }

    impl Endpoint for ProbeEndpoint {
        fn uri(&self) -> &str {
            &self.uri
        }

        fn create_consumer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
        ) -> Result<Box<dyn camel_component_api::Consumer>, CamelError> {
            Err(CamelError::EndpointCreationFailed("not a consumer".into()))
        }

        fn create_producer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
            _ctx: &ProducerContext,
        ) -> Result<BoxProcessor, CamelError> {
            let inflight = Arc::clone(&self.inflight);
            let max_inflight = Arc::clone(&self.max_inflight);
            Ok(BoxProcessor::from_fn(move |ex| {
                let inflight = Arc::clone(&inflight);
                let max_inflight = Arc::clone(&max_inflight);
                Box::pin(async move {
                    let current = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_inflight.fetch_max(current, Ordering::SeqCst);
                    // allow-test-sleep: 50ms window keeps concurrent probes
                    // overlapping long enough for the concurrency limit to be
                    // observable.
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    inflight.fetch_sub(1, Ordering::SeqCst);
                    Ok(ex)
                })
            }))
        }

        fn body_contract(&self) -> Option<BodyType> {
            Some(BodyType::Text)
        }

        fn lifecycle(&self) -> Option<Arc<dyn StepLifecycle>> {
            None
        }
    }

    /// Component that vends `ProbeEndpoint` for the `test:` scheme.
    struct ProbeComponent {
        inflight: Arc<AtomicUsize>,
        max_inflight: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Component for ProbeComponent {
        fn scheme(&self) -> &str {
            "test"
        }

        fn create_endpoint(
            &self,
            uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(ProbeEndpoint {
                uri: uri.to_string(),
                inflight: Arc::clone(&self.inflight),
                max_inflight: Arc::clone(&self.max_inflight),
            }))
        }
    }

    /// Producer whose `poll_ready` always fails — used to prove that the
    /// `LayeredTo` wrap does not swallow readiness errors.
    #[derive(Clone)]
    struct NeverReadyProducer;

    impl Service<Exchange> for NeverReadyProducer {
        type Response = Exchange;
        type Error = CamelError;
        type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Err(CamelError::EndpointCreationFailed(
                "probe not ready".into(),
            )))
        }

        fn call(&mut self, exchange: Exchange) -> Self::Future {
            Box::pin(async move { Ok(exchange) })
        }
    }

    struct NeverReadyEndpoint {
        uri: String,
    }

    impl Endpoint for NeverReadyEndpoint {
        fn uri(&self) -> &str {
            &self.uri
        }

        fn create_consumer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
        ) -> Result<Box<dyn camel_component_api::Consumer>, CamelError> {
            Err(CamelError::EndpointCreationFailed("not a consumer".into()))
        }

        fn create_producer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
            _ctx: &ProducerContext,
        ) -> Result<BoxProcessor, CamelError> {
            Ok(BoxProcessor::new(NeverReadyProducer))
        }
    }

    struct NeverReadyComponent;

    #[async_trait]
    impl Component for NeverReadyComponent {
        fn scheme(&self) -> &str {
            "neverready"
        }

        fn create_endpoint(
            &self,
            uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(NeverReadyEndpoint {
                uri: uri.to_string(),
            }))
        }
    }

    /// Component that tags its producer's output with the serving scheme.
    ///
    /// Used by the SkipTo interception test: the compiled process must be served
    /// by the intercept TARGET, not the authored scheme.
    struct TaggedComponent {
        scheme: &'static str,
    }

    struct TaggedEndpoint {
        uri: String,
        scheme: &'static str,
    }

    impl Endpoint for TaggedEndpoint {
        fn uri(&self) -> &str {
            &self.uri
        }

        fn create_consumer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
        ) -> Result<Box<dyn camel_component_api::Consumer>, CamelError> {
            Err(CamelError::EndpointCreationFailed("not a consumer".into()))
        }

        fn create_producer(
            &self,
            _rt: Arc<dyn RuntimeObservability>,
            _ctx: &ProducerContext,
        ) -> Result<BoxProcessor, CamelError> {
            let tag = self.scheme;
            Ok(BoxProcessor::from_fn(move |mut ex: Exchange| {
                Box::pin(async move {
                    ex.set_property("served_by", tag);
                    Ok(ex)
                })
            }))
        }
    }

    #[async_trait]
    impl Component for TaggedComponent {
        fn scheme(&self) -> &str {
            self.scheme
        }

        fn create_endpoint(
            &self,
            uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(TaggedEndpoint {
                uri: uri.to_string(),
                scheme: self.scheme,
            }))
        }
    }

    /// Build a `CompilationContext` that can resolve `stateful:dest` URIs.
    #[allow(clippy::too_many_arguments)]
    fn make_ctx<'a>(
        pc: &'a ProducerContext,
        rt: Arc<dyn RuntimeObservability>,
        languages: &'a super::super::SharedLanguageRegistry,
        beans: &'a Arc<Mutex<BeanRegistry>>,
        component_ctx: Arc<dyn ComponentContext>,
        staging: &'a FunctionStagingMode,
        idempotent_repositories: &'a IdempotentRegistry,
        claim_check_repositories: &'a ClaimCheckRegistry,
        cache_repositories: &'a crate::CacheRegistry,
    ) -> CompilationContext<'a> {
        CompilationContext {
            producer_ctx: pc,
            rt,
            languages,
            beans,
            function_invoker: None,
            component_ctx,
            route_id: None,
            staging_mode: staging,
            idempotent_repositories,
            claim_check_repositories,
            cache_repositories,
            intercept: crate::intercept::InterceptRules::default(),
        }
    }

    /// `To` arm must capture the endpoint's lifecycle handle into the
    /// produced `CompiledStep::Process` so the route controller can start
    /// and shut down stateful producers in route order.
    #[tokio::test]
    async fn to_arm_propagates_lifecycle() {
        let handle: Arc<dyn StepLifecycle> = Arc::new(FakeStep);
        let pc = ProducerContext::default();
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let languages: super::super::SharedLanguageRegistry = Arc::new(Mutex::new(HashMap::new()));
        let beans: Arc<Mutex<BeanRegistry>> = Arc::new(Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> = stateful_context(handle.clone());
        let staging = FunctionStagingMode::DirectAdd;
        let idempotent_repositories = IdempotentRegistry::new();
        let claim_check_repositories = ClaimCheckRegistry::new();
        let cache_repositories = crate::CacheRegistry::new();

        let ctx = make_ctx(
            &pc,
            rt,
            &languages,
            &beans,
            component_ctx,
            &staging,
            &idempotent_repositories,
            &claim_check_repositories,
            &cache_repositories,
        );

        // Sanity: the URI parses under the `stateful:` scheme.
        let parsed = parse_uri("stateful:dest").expect("uri parses");
        assert_eq!(parsed.scheme, "stateful");

        let compiled = EndpointsCompiler
            .compile(
                BuilderStep::To("stateful:dest".into()),
                0,
                &ctx,
                &StepCompilerRegistry::new(),
            )
            .expect("compilation should succeed");
        let step = match compiled {
            CompileOutcome::Matched(s) => s,
            CompileOutcome::NotHandled(_) => panic!("To must be handled"),
        };

        match step {
            CompiledStep::Process { lifecycle, .. } => {
                let lc = lifecycle.expect("Process.lifecycle should be Some for stateful endpoint");
                assert_eq!(
                    lc.name(),
                    "fake",
                    "propagated handle should be the FakeStep we registered"
                );
            }
            other => panic!("expected CompiledStep::Process, got {other:?}"),
        }
    }

    /// `WireTap` arm must also capture the endpoint's lifecycle handle.
    /// Uses the parallel `resolve_producer_with_lifecycle` helper under the
    /// hood (the bare `resolve_producer` does not surface the endpoint).
    #[tokio::test]
    async fn wiretap_arm_propagates_lifecycle() {
        let handle: Arc<dyn StepLifecycle> = Arc::new(FakeStep);
        let pc = ProducerContext::default();
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let languages: super::super::SharedLanguageRegistry = Arc::new(Mutex::new(HashMap::new()));
        let beans: Arc<Mutex<BeanRegistry>> = Arc::new(Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> = stateful_context(handle.clone());
        let staging = FunctionStagingMode::DirectAdd;
        let idempotent_repositories = IdempotentRegistry::new();
        let claim_check_repositories = ClaimCheckRegistry::new();
        let cache_repositories = crate::CacheRegistry::new();

        let ctx = make_ctx(
            &pc,
            rt,
            &languages,
            &beans,
            component_ctx,
            &staging,
            &idempotent_repositories,
            &claim_check_repositories,
            &cache_repositories,
        );

        let compiled = EndpointsCompiler
            .compile(
                BuilderStep::WireTap {
                    uri: "stateful:dest".into(),
                },
                0,
                &ctx,
                &StepCompilerRegistry::new(),
            )
            .expect("compilation should succeed");
        let step = match compiled {
            CompileOutcome::Matched(s) => s,
            CompileOutcome::NotHandled(_) => panic!("WireTap must be handled"),
        };

        match step {
            CompiledStep::Process { lifecycle, .. } => {
                let lc = lifecycle
                    .expect("Process.lifecycle should be Some for stateful WireTap endpoint");
                // The endpoint lifecycle is now composed with the WireTap
                // lifecycle via CompositeStepLifecycle → name is "composite".
                assert_eq!(lc.name(), "composite");
            }
            other => panic!("expected CompiledStep::Process, got {other:?}"),
        }
    }

    /// WireTap arm composes endpoint + WireTap lifecycles; shutdown drains
    /// both. Uses a shutdown-tracking fake to verify the endpoint handle is
    /// reached.
    #[tokio::test]
    async fn test_wiretap_compiler_composes_endpoint_and_wiretap_lifecycles() {
        let shutdown_count: Arc<std::sync::atomic::AtomicUsize> =
            Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handle: Arc<dyn StepLifecycle> = Arc::new(ShutdownTrackingFake {
            count: Arc::clone(&shutdown_count),
        });
        let pc = ProducerContext::default();
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let languages: super::super::SharedLanguageRegistry = Arc::new(Mutex::new(HashMap::new()));
        let beans: Arc<Mutex<BeanRegistry>> = Arc::new(Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> = stateful_context(handle.clone());
        let staging = FunctionStagingMode::DirectAdd;
        let idempotent_repositories = IdempotentRegistry::new();
        let claim_check_repositories = ClaimCheckRegistry::new();
        let cache_repositories = crate::CacheRegistry::new();

        let ctx = make_ctx(
            &pc,
            rt,
            &languages,
            &beans,
            component_ctx,
            &staging,
            &idempotent_repositories,
            &claim_check_repositories,
            &cache_repositories,
        );

        let compiled = EndpointsCompiler
            .compile(
                BuilderStep::WireTap {
                    uri: "stateful:dest".into(),
                },
                0,
                &ctx,
                &StepCompilerRegistry::new(),
            )
            .expect("compilation should succeed");
        let step = match compiled {
            CompileOutcome::Matched(s) => s,
            CompileOutcome::NotHandled(_) => panic!("WireTap must be handled"),
        };

        let lifecycle = match step {
            CompiledStep::Process { lifecycle, .. } => {
                lifecycle.expect("Process.lifecycle should be Some")
            }
            other => panic!("expected CompiledStep::Process, got {other:?}"),
        };

        // Shutdown should reach the endpoint fake (reverse order: WireTap
        // first, then endpoint).
        lifecycle
            .shutdown(StepShutdownReason::RouteStop)
            .await
            .expect("shutdown should succeed");
        assert_eq!(
            shutdown_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "endpoint lifecycle shutdown should have been called once"
        );
    }

    /// WireTap arm when the endpoint has no lifecycle handle: the compiled
    /// step still gets a lifecycle (the WireTap-only handle).
    #[tokio::test]
    async fn test_wiretap_compiler_compose_when_no_endpoint_lifecycle() {
        let pc = ProducerContext::default();
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let languages: super::super::SharedLanguageRegistry = Arc::new(Mutex::new(HashMap::new()));
        let beans: Arc<Mutex<BeanRegistry>> = Arc::new(Mutex::new(BeanRegistry::new()));
        // Use a stateless component whose endpoints return None from lifecycle().
        let component_ctx: Arc<dyn ComponentContext> = stateless_context();
        let staging = FunctionStagingMode::DirectAdd;
        let idempotent_repositories = IdempotentRegistry::new();
        let claim_check_repositories = ClaimCheckRegistry::new();
        let cache_repositories = crate::CacheRegistry::new();

        let ctx = make_ctx(
            &pc,
            rt,
            &languages,
            &beans,
            component_ctx,
            &staging,
            &idempotent_repositories,
            &claim_check_repositories,
            &cache_repositories,
        );

        let compiled = EndpointsCompiler
            .compile(
                BuilderStep::WireTap {
                    uri: "stateless:dest".into(),
                },
                0,
                &ctx,
                &StepCompilerRegistry::new(),
            )
            .expect("compilation should succeed");
        let step = match compiled {
            CompileOutcome::Matched(s) => s,
            CompileOutcome::NotHandled(_) => panic!("WireTap must be handled"),
        };

        match step {
            CompiledStep::Process { lifecycle, .. } => {
                let lc = lifecycle.expect("Process.lifecycle should be Some (WireTap-only)");
                // WireTap-only lifecycle → name is "wiretap".
                assert_eq!(lc.name(), "wiretap");
                // Shutdown should succeed (drains the WireTap handle).
                lc.shutdown(StepShutdownReason::RouteStop)
                    .await
                    .expect("WireTap-only shutdown should succeed");
            }
            other => panic!("expected CompiledStep::Process, got {other:?}"),
        }
    }

    /// `LayeredTo` compiles to the SAME `Process` shape as `To` (body
    /// contract, lifecycle presence) and the layer is engaged around the
    /// resolved producer: with `ConcurrencyLimitLayer(4)` and six concurrent
    /// invocations the producer never sees more than four in flight.
    #[tokio::test]
    async fn layered_to_compiles_with_to_parity() {
        let inflight = Arc::new(AtomicUsize::new(0));
        let max_inflight = Arc::new(AtomicUsize::new(0));
        let pc = ProducerContext::default();
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let languages: super::super::SharedLanguageRegistry = Arc::new(Mutex::new(HashMap::new()));
        let beans: Arc<Mutex<BeanRegistry>> = Arc::new(Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> =
            probe_context(Arc::clone(&inflight), Arc::clone(&max_inflight));
        let staging = FunctionStagingMode::DirectAdd;
        let idempotent_repositories = IdempotentRegistry::new();
        let claim_check_repositories = ClaimCheckRegistry::new();
        let cache_repositories = crate::CacheRegistry::new();

        let ctx = make_ctx(
            &pc,
            rt,
            &languages,
            &beans,
            component_ctx,
            &staging,
            &idempotent_repositories,
            &claim_check_repositories,
            &cache_repositories,
        );

        let to_step = match EndpointsCompiler
            .compile(
                BuilderStep::To("test:out".into()),
                0,
                &ctx,
                &StepCompilerRegistry::new(),
            )
            .expect("To compilation should succeed")
        {
            CompileOutcome::Matched(s) => s,
            CompileOutcome::NotHandled(_) => panic!("To must be handled"),
        };

        let layered_step = match EndpointsCompiler
            .compile(
                BuilderStep::LayeredTo {
                    uri: "test:out".into(),
                    factory: LayerFactory::new(tower::limit::ConcurrencyLimitLayer::new(4)),
                },
                0,
                &ctx,
                &StepCompilerRegistry::new(),
            )
            .expect("LayeredTo compilation should succeed")
        {
            CompileOutcome::Matched(s) => s,
            CompileOutcome::NotHandled(_) => panic!("LayeredTo must be handled"),
        };

        let (to_body, to_lifecycle) = match to_step {
            CompiledStep::Process {
                body_contract,
                lifecycle,
                ..
            } => (body_contract, lifecycle.is_some()),
            other => panic!("expected CompiledStep::Process for To, got {other:?}"),
        };
        let (processor, layered_body, layered_lifecycle) = match layered_step {
            CompiledStep::Process {
                processor,
                body_contract,
                lifecycle,
                ..
            } => (processor, body_contract, lifecycle.is_some()),
            other => panic!("expected CompiledStep::Process for LayeredTo, got {other:?}"),
        };

        assert_eq!(
            to_body, layered_body,
            "LayeredTo must carry the same body contract as plain To"
        );
        assert!(
            to_body.is_some(),
            "parity assertion is only meaningful when the endpoint declares a body contract"
        );
        assert_eq!(
            to_lifecycle, layered_lifecycle,
            "LayeredTo must preserve lifecycle presence semantics of plain To"
        );

        // The producer sleeps 50ms, so six concurrent oneshot calls must
        // saturate — and never exceed — the four permits.
        let calls: Vec<_> = (0..6)
            .map(|i| {
                let processor = processor.clone();
                async move {
                    ServiceExt::oneshot(processor, Exchange::new(Message::new(format!("m{i}"))))
                        .await
                }
            })
            .collect();
        for result in futures::future::join_all(calls).await {
            result.expect("probe call should succeed");
        }
        let observed = max_inflight.load(Ordering::SeqCst);
        assert_eq!(
            observed, 4,
            "six concurrent calls must engage the limit at exactly 4 in flight"
        );
        assert_eq!(
            inflight.load(Ordering::SeqCst),
            0,
            "all in-flight probes must have completed"
        );
    }

    /// Wrap failures propagate: a producer whose `poll_ready` errors must
    /// surface that error through the wrapped `BoxProcessor`, not get
    /// swallowed into `Ready(Ok(()))`.
    #[tokio::test]
    async fn layered_to_propagates_readiness_error() {
        let pc = ProducerContext::default();
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let languages: super::super::SharedLanguageRegistry = Arc::new(Mutex::new(HashMap::new()));
        let beans: Arc<Mutex<BeanRegistry>> = Arc::new(Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> = never_ready_context();
        let staging = FunctionStagingMode::DirectAdd;
        let idempotent_repositories = IdempotentRegistry::new();
        let claim_check_repositories = ClaimCheckRegistry::new();
        let cache_repositories = crate::CacheRegistry::new();

        let ctx = make_ctx(
            &pc,
            rt,
            &languages,
            &beans,
            component_ctx,
            &staging,
            &idempotent_repositories,
            &claim_check_repositories,
            &cache_repositories,
        );

        let step = match EndpointsCompiler
            .compile(
                BuilderStep::LayeredTo {
                    uri: "neverready:out".into(),
                    factory: LayerFactory::new(tower::limit::ConcurrencyLimitLayer::new(1)),
                },
                0,
                &ctx,
                &StepCompilerRegistry::new(),
            )
            .expect("LayeredTo compilation should succeed")
        {
            CompileOutcome::Matched(s) => s,
            CompileOutcome::NotHandled(_) => panic!("LayeredTo must be handled"),
        };
        let mut processor = match step {
            CompiledStep::Process { processor, .. } => processor,
            other => panic!("expected CompiledStep::Process, got {other:?}"),
        };

        let err = std::future::poll_fn(|cx| tower::Service::poll_ready(&mut processor, cx))
            .await
            .expect_err("readiness error must surface through the wrapped processor");
        assert!(
            err.to_string().contains("probe not ready"),
            "unexpected readiness error: {err}"
        );
    }

    /// `DivertCopyTo` has no layered counterpart: the arm must fail loudly
    /// instead of silently downgrading the divert to a plain send.
    #[tokio::test]
    async fn layered_to_rejects_divert_interception() {
        let pc = ProducerContext::default();
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let languages: super::super::SharedLanguageRegistry = Arc::new(Mutex::new(HashMap::new()));
        let beans: Arc<Mutex<BeanRegistry>> = Arc::new(Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> = stateless_context();
        let staging = FunctionStagingMode::DirectAdd;
        let idempotent_repositories = IdempotentRegistry::new();
        let claim_check_repositories = ClaimCheckRegistry::new();
        let cache_repositories = crate::CacheRegistry::new();

        let mut ctx = make_ctx(
            &pc,
            rt,
            &languages,
            &beans,
            component_ctx,
            &staging,
            &idempotent_repositories,
            &claim_check_repositories,
            &cache_repositories,
        );
        ctx.intercept =
            crate::intercept::InterceptRules::new(vec![crate::intercept::InterceptRule {
                uri: "test:out".into(),
                action: InterceptAction::DivertCopyTo {
                    uri: "mock:copy".into(),
                },
            }])
            .expect("a mock: divert target is a valid rule");

        let err = match EndpointsCompiler.compile(
            BuilderStep::LayeredTo {
                uri: "test:out".into(),
                factory: LayerFactory::new(tower::limit::ConcurrencyLimitLayer::new(1)),
            },
            0,
            &ctx,
            &StepCompilerRegistry::new(),
        ) {
            Ok(_) => panic!("LayeredTo under DivertCopyTo must be rejected"),
            Err(e) => e,
        };
        match err {
            CamelError::RouteError(msg) => {
                assert!(
                    msg.contains("to_layered"),
                    "error must name the layered step: {msg}"
                );
                assert!(
                    msg.contains("DivertCopyTo"),
                    "error must name the unsupported interception: {msg}"
                );
            }
            other => panic!("expected CamelError::RouteError, got {other:?}"),
        }
    }

    /// `LayeredTo` must honour `SkipTo` interception exactly like `To`: the
    /// compiled Process wraps the TARGET producer, so a run is served by the
    /// intercept target (here `mock:`), not the authored URI's scheme.
    #[tokio::test]
    async fn layered_to_compiles_under_skip_to_interception() {
        let pc = ProducerContext::default();
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoopRuntimeObservability);
        let languages: super::super::SharedLanguageRegistry = Arc::new(Mutex::new(HashMap::new()));
        let beans: Arc<Mutex<BeanRegistry>> = Arc::new(Mutex::new(BeanRegistry::new()));
        let component_ctx: Arc<dyn ComponentContext> = Arc::new(
            TestCompilerContext::new()
                .with(Arc::new(TaggedComponent { scheme: "test" }))
                .with(Arc::new(TaggedComponent { scheme: "mock" })),
        );
        let staging = FunctionStagingMode::DirectAdd;
        let idempotent_repositories = IdempotentRegistry::new();
        let claim_check_repositories = ClaimCheckRegistry::new();
        let cache_repositories = crate::CacheRegistry::new();

        let mut ctx = make_ctx(
            &pc,
            rt,
            &languages,
            &beans,
            component_ctx,
            &staging,
            &idempotent_repositories,
            &claim_check_repositories,
            &cache_repositories,
        );
        ctx.intercept =
            crate::intercept::InterceptRules::new(vec![crate::intercept::InterceptRule {
                uri: "test:out".into(),
                action: InterceptAction::SkipTo {
                    uri: "mock:target".into(),
                },
            }])
            .expect("a mock: intercept target is a valid rule");

        let step = match EndpointsCompiler
            .compile(
                BuilderStep::LayeredTo {
                    uri: "test:out".into(),
                    factory: LayerFactory::new(tower::limit::ConcurrencyLimitLayer::new(4)),
                },
                0,
                &ctx,
                &StepCompilerRegistry::new(),
            )
            .expect("LayeredTo compilation should succeed")
        {
            CompileOutcome::Matched(s) => s,
            CompileOutcome::NotHandled(_) => panic!("LayeredTo must be handled"),
        };
        let processor = match step {
            CompiledStep::Process { processor, .. } => processor,
            other => panic!("expected CompiledStep::Process, got {other:?}"),
        };

        let ex = ServiceExt::oneshot(processor, Exchange::new(Message::new("x")))
            .await
            .expect("the wrapped intercept-target producer should succeed");
        assert_eq!(
            ex.property("served_by").and_then(|v| v.as_str()),
            Some("mock"),
            "SkipTo must substitute the target before the LayeredTo wrap"
        );
    }
}
