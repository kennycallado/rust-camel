//! Route compilation extraction — [`RouteCompilerExt`] holds borrowed references
//! from [`DefaultRouteController`](super::route_controller::DefaultRouteController) to
//! compile route definitions into processor pipelines.
//!
//! This module also exposes [`resolve_error_handler`] and [`resolve_uow_layer`] as
//! public free functions so that [`DefaultRouteController`] can call them without
//! duplicating the implementations.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Weak};

use camel_api::circuit_breaker::CircuitBreakerConfig;
use camel_api::error_handler::{ErrorHandlerConfig, ExceptionDisposition, ExceptionPolicy};
use camel_api::metrics::MetricsCollector;
use camel_api::security_policy::SecurityPolicyConfig;
use camel_api::{
    BoxProcessor, CamelError, FunctionInvoker, IdentityProcessor, NoOpMetrics, PlatformService,
    ProducerContext, RuntimeHandle, UnitOfWorkConfig,
};
use camel_bean::BeanRegistry;
use camel_component_api::{ComponentContext, RuntimeObservability};
use camel_processor::aggregator::SharedLanguageRegistry;
use camel_processor::circuit_breaker::CircuitBreakerGate;
use camel_processor::circuit_breaker::CircuitBreakerLayer;
use camel_processor::error_handler::{DefaultRouteErrorHandler, RouteErrorHandler};
use camel_processor::security_policy_layer::SecurityPolicyLayer;
use tower::Layer;

use crate::health_registry::HealthCheckRegistry;
use crate::lifecycle::adapters::controller_component_context::ControllerComponentContext;
use crate::lifecycle::adapters::exchange_uow::ExchangeUoWLayer;
use crate::lifecycle::adapters::route_compiler::{
    RouteChannelService, compose_traced_pipeline_with_contracts,
};
use crate::lifecycle::adapters::route_registry::RouteRegistry;
use crate::lifecycle::application::route_definition::{BuilderStep, RouteDefinition};
use crate::shared::components::domain::Registry;
use crate::shared::observability::domain::DetailLevel;

// ── Free functions (shared between RouteCompilerExt and DefaultRouteController) ──

/// Resolve an `ErrorHandlerConfig` into a `DefaultRouteErrorHandler`.
pub(super) fn resolve_error_handler(
    config: ErrorHandlerConfig,
    producer_ctx: &ProducerContext,
    rt: Arc<dyn camel_component_api::RuntimeObservability>,
    component_ctx: &dyn ComponentContext,
) -> Result<DefaultRouteErrorHandler, CamelError> {
    // Resolve DLC URI → producer.
    let dlc_producer = if let Some(ref uri) = config.dlc_uri {
        let parsed = camel_endpoint::parse_uri(uri)?;
        let component = component_ctx
            .resolve_component(&parsed.scheme)
            .ok_or_else(|| CamelError::ComponentNotFound(parsed.scheme.clone()))?;
        let endpoint = component.create_endpoint(uri, component_ctx)?;
        Some(endpoint.create_producer(Arc::clone(&rt), producer_ctx)?)
    } else {
        None
    };

    // Backward compat: when a DLC is configured with no explicit policies,
    // add a catch-all Handled policy so errors are absorbed (old behavior).
    let policies = if config.policies.is_empty() && dlc_producer.is_some() {
        vec![ExceptionPolicy {
            matches: Arc::new(|_| true),
            retry: None,
            handled_by: None,
            on_steps: None,
            disposition: ExceptionDisposition::Handled,
        }]
    } else {
        config.policies
    };

    // Resolve per-policy `handled_by` URIs.
    let mut resolved_policies = Vec::new();
    for policy in policies {
        let handler_producer = if let Some(ref uri) = policy.handled_by {
            let parsed = camel_endpoint::parse_uri(uri)?;
            let component = component_ctx
                .resolve_component(&parsed.scheme)
                .ok_or_else(|| CamelError::ComponentNotFound(parsed.scheme.clone()))?;
            let endpoint = component.create_endpoint(uri, component_ctx)?;
            Some(endpoint.create_producer(Arc::clone(&rt), producer_ctx)?)
        } else {
            None
        };
        resolved_policies.push((policy, handler_producer));
    }

    Ok(DefaultRouteErrorHandler::new(
        dlc_producer,
        resolved_policies,
    ))
}

/// Resolve a `UnitOfWorkConfig` into an `(ExchangeUoWLayer, Arc<AtomicU64>)`.
/// Returns `Err` if any hook URI cannot be resolved.
pub(super) fn resolve_uow_layer(
    config: &UnitOfWorkConfig,
    producer_ctx: &ProducerContext,
    rt: Arc<dyn camel_component_api::RuntimeObservability>,
    component_ctx: &dyn ComponentContext,
    counter: Option<Arc<AtomicU64>>,
) -> Result<(ExchangeUoWLayer, Arc<AtomicU64>), CamelError> {
    let resolve_uri = |uri: &str| -> Result<BoxProcessor, CamelError> {
        let parsed = camel_endpoint::parse_uri(uri)?;
        let component = component_ctx
            .resolve_component(&parsed.scheme)
            .ok_or_else(|| CamelError::ComponentNotFound(parsed.scheme.clone()))?;
        let endpoint = component.create_endpoint(uri, component_ctx)?;
        endpoint
            .create_producer(Arc::clone(&rt), producer_ctx)
            .map_err(|e| {
                CamelError::RouteError(format!("UoW hook URI '{uri}' could not be resolved: {e}"))
            })
    };

    let on_complete = config.on_complete.as_deref().map(resolve_uri).transpose()?;
    let on_failure = config.on_failure.as_deref().map(resolve_uri).transpose()?;

    let counter = counter.unwrap_or_else(|| Arc::new(AtomicU64::new(0)));
    let layer = ExchangeUoWLayer::new(Arc::clone(&counter), on_complete, on_failure);
    Ok((layer, counter))
}

/// Build a pipeline with or without an error handler + RouteChannelService.
///
/// When `eh_config` is `Some`, constructs a [`RouteChannelService`] with explicit
/// security and circuit-breaker gates, and the handler injected into the pipeline
/// for step-level error recovery.
///
/// When `eh_config` is `None`, falls back to Tower layer wrapping for circuit
/// breaker and security (no error handler).
///
/// # Parameters
///
/// The large number of parameters is justified because they're all needed by one
/// of the two branches (eh_config present/absent) and extracting groups would add
/// more complexity than it removes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_eh_config_pipeline(
    eh_config: Option<&ErrorHandlerConfig>,
    registry: Arc<std::sync::Mutex<Registry>>,
    languages: SharedLanguageRegistry,
    tracer_metrics: Option<Arc<dyn MetricsCollector>>,
    platform_service: Arc<dyn PlatformService>,
    health_registry: Arc<HealthCheckRegistry>,
    route_id: &str,
    producer_ctx: &ProducerContext,
    processors_with_contracts: Vec<(BoxProcessor, Option<camel_api::BodyType>)>,
    tracing_enabled: bool,
    tracer_detail_level: DetailLevel,
    security_policy: Option<SecurityPolicyConfig>,
    circuit_breaker: Option<CircuitBreakerConfig>,
) -> Result<BoxProcessor, CamelError> {
    Ok(if let Some(config) = eh_config {
        // ── New path: RouteChannelService with explicit gates ──
        let component_ctx = Arc::new(ControllerComponentContext::new(
            registry,
            languages,
            tracer_metrics
                .clone()
                .unwrap_or_else(|| Arc::new(NoOpMetrics)),
            platform_service,
            health_registry,
            Some(route_id.to_string()),
        ));
        let rt: Arc<dyn RuntimeObservability> = Arc::clone(&component_ctx) as Arc<_>;
        let handler = Arc::new(resolve_error_handler(
            config.clone(),
            producer_ctx,
            rt,
            component_ctx.as_ref(),
        )?) as Arc<dyn RouteErrorHandler>;

        let pipeline = compose_traced_pipeline_with_contracts(
            processors_with_contracts,
            route_id,
            tracing_enabled,
            tracer_detail_level,
            tracer_metrics,
            Some(handler.clone()),
        );

        // Security: standalone gate (SecurityPolicyLayer wrapping IdentityProcessor)
        let security = security_policy.map(|sp| {
            BoxProcessor::new(SecurityPolicyLayer::new(sp.policy).layer(IdentityProcessor))
        });

        // CircuitBreaker: explicit gate
        let cb_gate = circuit_breaker.map(CircuitBreakerGate::new);

        let channel = RouteChannelService::new(handler, security, cb_gate, pipeline);
        BoxProcessor::new(channel)
    } else {
        // ── Old path: Tower layer wrapping (no error handler configured) ──
        let mut pipeline = compose_traced_pipeline_with_contracts(
            processors_with_contracts,
            route_id,
            tracing_enabled,
            tracer_detail_level,
            tracer_metrics,
            None,
        );

        if let Some(cb_config) = circuit_breaker {
            let cb_layer = CircuitBreakerLayer::new(cb_config);
            pipeline = BoxProcessor::new(cb_layer.layer(pipeline));
        }

        if let Some(sp_config) = security_policy {
            let sp_layer = SecurityPolicyLayer::new(sp_config.policy);
            pipeline = BoxProcessor::new(sp_layer.layer(pipeline));
        }

        pipeline
    })
}

// ── RouteCompilerExt ──

/// A borrowed-context struct that holds references to the fields needed for
/// route compilation. Created transiently inside `DefaultRouteController` methods.
pub(crate) struct RouteCompilerExt<'a> {
    pub(crate) registry: &'a Arc<std::sync::Mutex<Registry>>,
    pub(crate) languages: &'a SharedLanguageRegistry,
    pub(crate) beans: &'a Arc<std::sync::Mutex<BeanRegistry>>,
    pub(crate) function_invoker: &'a Option<Arc<dyn FunctionInvoker>>,
    pub(crate) tracing_enabled: bool,
    pub(crate) tracer_detail_level: &'a DetailLevel,
    pub(crate) tracer_metrics: &'a Option<Arc<dyn MetricsCollector>>,
    pub(crate) platform_service: &'a Arc<dyn PlatformService>,
    pub(crate) runtime: &'a Option<Weak<dyn RuntimeHandle>>,
    pub(crate) global_error_handler: &'a Option<ErrorHandlerConfig>,
    pub(crate) health_registry: &'a Option<Arc<HealthCheckRegistry>>,
    pub(crate) route_registry: &'a RouteRegistry,
}

impl RouteCompilerExt<'_> {
    fn health_registry(&self) -> Arc<HealthCheckRegistry> {
        self.health_registry.clone().unwrap_or_else(|| {
            tracing::warn!("health_registry not configured — creating isolated fallback");
            Arc::new(HealthCheckRegistry::new(std::time::Duration::from_secs(5)))
        })
    }

    fn build_producer_context(&self, route_id: &str) -> Result<ProducerContext, CamelError> {
        let mut producer_ctx = ProducerContext::new().with_route_id(route_id);
        if let Some(runtime) = self.runtime.as_ref().and_then(Weak::upgrade) {
            producer_ctx = producer_ctx.with_runtime(runtime);
        }
        Ok(producer_ctx)
    }

    fn resolve_steps(
        &self,
        steps: Vec<BuilderStep>,
        producer_ctx: &ProducerContext,
        registry: &Arc<std::sync::Mutex<Registry>>,
        route_id: Option<&str>,
        staging_mode: &super::step_resolution::FunctionStagingMode,
    ) -> Result<Vec<(BoxProcessor, Option<camel_api::BodyType>)>, CamelError> {
        let component_ctx = Arc::new(ControllerComponentContext::new(
            Arc::clone(registry),
            Arc::clone(self.languages),
            self.tracer_metrics
                .clone()
                .unwrap_or_else(|| Arc::new(NoOpMetrics)),
            Arc::clone(self.platform_service),
            self.health_registry(),
            route_id.map(|s| s.to_string()),
        ));
        let rt: Arc<dyn camel_component_api::RuntimeObservability> =
            Arc::clone(&component_ctx) as Arc<_>;

        super::step_resolution::resolve_steps(
            steps,
            producer_ctx,
            rt,
            registry,
            self.languages,
            self.beans,
            self.function_invoker.clone(),
            component_ctx,
            route_id,
            staging_mode,
        )
    }

    /// Shared implementation body for route compilation.
    /// The only difference between `compile_route_definition` and
    /// `compile_route_definition_with_generation` is the `FunctionStagingMode`.
    fn compile_route_impl(
        &self,
        def: RouteDefinition,
        staging_mode: &super::step_resolution::FunctionStagingMode,
    ) -> Result<BoxProcessor, CamelError> {
        let route_id = def.route_id().to_string();

        let producer_ctx = self.build_producer_context(&route_id)?;

        let processors_with_contracts = self.resolve_steps(
            def.steps,
            &producer_ctx,
            self.registry,
            Some(&route_id),
            staging_mode,
        )?;

        let eh_config = def
            .error_handler
            .clone()
            .or_else(|| self.global_error_handler.clone());

        let mut pipeline = build_eh_config_pipeline(
            eh_config.as_ref(),
            Arc::clone(self.registry),
            Arc::clone(self.languages),
            self.tracer_metrics.clone(),
            Arc::clone(self.platform_service),
            self.health_registry(),
            &route_id,
            &producer_ctx,
            processors_with_contracts,
            self.tracing_enabled,
            self.tracer_detail_level.clone(),
            def.security_policy,
            def.circuit_breaker,
        )?;

        // Apply UoW layer outermost
        if let Some(uow_config) = &def.unit_of_work {
            let existing_counter = self.route_registry.in_flight_counter(&route_id);

            let component_ctx = Arc::new(ControllerComponentContext::new(
                Arc::clone(self.registry),
                Arc::clone(self.languages),
                self.tracer_metrics
                    .clone()
                    .unwrap_or_else(|| Arc::new(NoOpMetrics)),
                Arc::clone(self.platform_service),
                self.health_registry(),
                Some(route_id.clone()),
            ));
            let rt: Arc<dyn camel_component_api::RuntimeObservability> =
                Arc::clone(&component_ctx) as Arc<_>;

            let (uow_layer, _counter) = resolve_uow_layer(
                uow_config,
                &producer_ctx,
                rt,
                component_ctx.as_ref(),
                existing_counter,
            )?;

            pipeline = BoxProcessor::new(uow_layer.layer(pipeline));
        }

        Ok(pipeline)
    }

    /// Compile a route definition into a processor pipeline, without adding it
    /// to the controller. Used for validation and testing.
    pub(crate) fn compile_route_definition(
        &self,
        def: RouteDefinition,
    ) -> Result<BoxProcessor, CamelError> {
        self.compile_route_impl(
            def,
            &super::step_resolution::FunctionStagingMode::DryCompile,
        )
    }

    /// Compile a route definition with a specific generation (for hot-reload).
    pub(crate) fn compile_route_definition_with_generation(
        &self,
        def: RouteDefinition,
        generation: u64,
    ) -> Result<BoxProcessor, CamelError> {
        self.compile_route_impl(
            def,
            &super::step_resolution::FunctionStagingMode::HotReload { generation },
        )
    }
}
