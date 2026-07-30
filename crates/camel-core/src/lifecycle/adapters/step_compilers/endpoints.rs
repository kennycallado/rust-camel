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
                let (producer, lifecycle) = resolve_producer_with_lifecycle(ctx, &uri)?;
                let svc = camel_processor::WireTapService::new(producer);
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

        let ctx = make_ctx(
            &pc,
            rt,
            &languages,
            &beans,
            component_ctx,
            &staging,
            &idempotent_repositories,
            &claim_check_repositories,
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

        let ctx = make_ctx(
            &pc,
            rt,
            &languages,
            &beans,
            component_ctx,
            &staging,
            &idempotent_repositories,
            &claim_check_repositories,
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
                assert_eq!(lc.name(), "fake");
            }
            other => panic!("expected CompiledStep::Process, got {other:?}"),
        }
    }
}
