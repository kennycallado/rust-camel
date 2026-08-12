//! Endpoint step compilers: To, WireTap.
//!
//! These steps create producers from URIs at compile time.

use std::sync::Arc;

use camel_api::{BoxProcessor, CamelError, StepLifecycle};

use camel_endpoint::parse_uri;

use super::{
    CompilationContext, CompileOutcome, CompiledStep, StepCompiler, StepCompilerRegistry,
    resolve_producer_with_lifecycle,
};
use crate::lifecycle::adapters::CompositeStepLifecycle;
use crate::lifecycle::application::route_definition::BuilderStep;

pub(crate) struct EndpointsCompiler;

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
                let parsed = parse_uri(&uri)?;
                let component = ctx
                    .component_ctx
                    .resolve_component(&parsed.scheme)
                    .ok_or_else(|| CamelError::ComponentNotFound(parsed.scheme.clone()))?;
                let endpoint = component.create_endpoint(&uri, ctx.component_ctx.as_ref())?;
                let contract = endpoint.body_contract();
                let producer = endpoint.create_producer(Arc::clone(&ctx.rt), ctx.producer_ctx)?;
                // Capture the endpoint's lifecycle handle so the route
                // controller can start/shut it down in route order (ADR-0022).
                // Default is `None` for stateless endpoints — see
                // `Endpoint::lifecycle` in camel-component-api.
                let lifecycle: Option<Arc<dyn StepLifecycle>> = endpoint.lifecycle();
                Ok(CompileOutcome::Matched(CompiledStep::Process {
                    processor: producer,
                    body_contract: contract,
                    lifecycle,
                }))
            }

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
                    processor: BoxProcessor::new(svc),
                    body_contract: None,
                    lifecycle,
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
    use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, StepLifecycle, StepShutdownReason};
    use camel_bean::BeanRegistry;
    use camel_component_api::{
        Component, ComponentContext, Endpoint, ProducerContext, RuntimeObservability,
        test_support::NoopRuntimeObservability,
    };
    use camel_endpoint::parse_uri;
    use std::collections::HashMap;
    use std::sync::Mutex;

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

    /// `ComponentContext` that resolves only the `stateful:` scheme.
    struct StatefulContext {
        handle: Arc<dyn StepLifecycle>,
    }

    impl ComponentContext for StatefulContext {
        fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>> {
            if scheme == "stateful" {
                Some(Arc::new(StatefulComponent {
                    handle: self.handle.clone(),
                }))
            } else {
                None
            }
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

    /// `ComponentContext` that resolves only the `stateless:` scheme.
    struct StatelessContext;

    impl ComponentContext for StatelessContext {
        fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>> {
            if scheme == "stateless" {
                Some(Arc::new(StatelessComponent))
            } else {
                None
            }
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
        let component_ctx: Arc<dyn ComponentContext> = Arc::new(StatefulContext {
            handle: handle.clone(),
        });
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
        let component_ctx: Arc<dyn ComponentContext> = Arc::new(StatefulContext {
            handle: handle.clone(),
        });
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
        let component_ctx: Arc<dyn ComponentContext> = Arc::new(StatefulContext {
            handle: handle.clone(),
        });
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
        let component_ctx: Arc<dyn ComponentContext> = Arc::new(StatelessContext);
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
}
