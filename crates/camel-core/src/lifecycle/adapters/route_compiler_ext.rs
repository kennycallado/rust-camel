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
    ProducerContext, RuntimeHandle, StepLifecycle, UnitOfWorkConfig,
};
use camel_bean::BeanRegistry;
use camel_component_api::{ComponentContext, RuntimeObservability};
use camel_processor::aggregator::SharedLanguageRegistry;
use camel_processor::circuit_breaker::CircuitBreakerGate;
use camel_processor::circuit_breaker::CircuitBreakerLayer;
use camel_processor::error_handler::{DefaultRouteErrorHandler, RouteErrorHandler};
use camel_processor::resequencer::{
    ResequencePolicy, ResequencerService, batch::BatchPolicy, stream::StreamPolicy,
};
use camel_processor::security_policy_layer::SecurityPolicyLayer;
use tower::Layer;

use crate::health_registry::HealthCheckRegistry;
use crate::lifecycle::adapters::controller_component_context::ControllerComponentContext;
use crate::lifecycle::adapters::exchange_uow::ExchangeUoWLayer;
use crate::lifecycle::adapters::route_compiler::{
    PipelineRuntimeCtx, RouteChannelService, compose_pipeline, compose_pipeline_with_contracts,
    compose_traced_pipeline_with_contracts,
};
use crate::lifecycle::adapters::route_helpers::{
    AggregateSplitInfo, find_top_level_aggregate_requiring_split,
    find_top_level_resequencer_requiring_split,
};
use crate::lifecycle::adapters::route_registry::RouteRegistry;
use crate::lifecycle::adapters::step_compilers::CompiledStep;
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

    Ok(
        DefaultRouteErrorHandler::new(dlc_producer, resolved_policies)
            .with_use_original_message(config.use_original_message),
    )
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

/// Build a `PipelineRuntimeCtx` from optional tracer metrics and a route ID.
///
/// Extracted to eliminate the same 5-line construction block that appeared 5×
/// across `build_eh_config_pipeline` (free fn) and `RouteCompilerExt` methods.
fn build_pipeline_ctx(
    tracer_metrics: &Option<Arc<dyn MetricsCollector>>,
    route_id: &str,
) -> PipelineRuntimeCtx {
    PipelineRuntimeCtx {
        metrics: tracer_metrics
            .clone()
            .unwrap_or_else(|| Arc::new(NoOpMetrics)),
        route_id: Arc::from(route_id),
    }
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
    processors_with_contracts: Vec<CompiledStep>,
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

        // Build ctx: real metrics from controller (or NoOpMetrics fallback) +
        // route_id. The cancel token is per-start via task-local, not here.
        let pipeline_ctx = build_pipeline_ctx(&tracer_metrics, route_id);

        let pipeline = compose_traced_pipeline_with_contracts(
            processors_with_contracts,
            route_id,
            tracing_enabled,
            tracer_detail_level,
            tracer_metrics,
            Some(handler.clone()),
            pipeline_ctx,
        );

        // Security: standalone gate (SecurityPolicyLayer wrapping IdentityProcessor)
        let security = security_policy.map(|sp| {
            BoxProcessor::new(SecurityPolicyLayer::new(sp.policy).layer(IdentityProcessor))
        });

        // CircuitBreaker: explicit gate
        let cb_gate = circuit_breaker.map(CircuitBreakerGate::new);

        let channel = RouteChannelService::new(
            handler,
            security,
            cb_gate,
            pipeline,
            config.use_original_message,
        );
        BoxProcessor::new(channel)
    } else {
        // ── Old path: Tower layer wrapping (no error handler configured) ──
        let pipeline_ctx = build_pipeline_ctx(&tracer_metrics, route_id);
        let mut pipeline = compose_traced_pipeline_with_contracts(
            processors_with_contracts,
            route_id,
            tracing_enabled,
            tracer_detail_level,
            tracer_metrics,
            None,
            pipeline_ctx,
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
    pub(crate) idempotent_repositories: crate::SharedIdempotentRegistry,
    pub(crate) claim_check_repositories: crate::SharedClaimCheckRegistry,
}

impl RouteCompilerExt<'_> {
    fn health_registry(&self) -> Arc<HealthCheckRegistry> {
        self.health_registry.clone().unwrap_or_else(|| {
            tracing::debug!("health_registry not configured — creating isolated fallback");
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
    ) -> Result<Vec<CompiledStep>, CamelError> {
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
            self.idempotent_repositories.as_ref(),
            self.claim_check_repositories.as_ref(),
        )
    }

    /// Detect whether a route definition contains a top-level aggregate or
    /// resequencer step that requires the split-processing pipeline.
    ///
    /// **Aggregate split:** builds pre/post [`SharedPipeline`]s and returns
    /// `AggregateSplitInfo`.
    ///
    /// **Resequencer split (Phase 3):** partitions into pre / resequencer /
    /// post, compiles pre normally, compiles post into a
    /// `BoxProcessor` continuation owned by a `ResequencerService`, and
    /// returns the main pipeline as `pre + [resequencer_step]`.  The whole
    /// route is ONE `PipelineAssembly`.
    ///
    /// Returns `(Option<AggregateSplitInfo>, Vec<CompiledStep>)`:
    /// - When aggregate split is detected, `Vec` is empty (split info carries the
    ///   pipelines).
    /// - When resequencer split is detected, `aggregate_split` is `None` and
    ///   `Vec` carries the compiled main-pipeline steps.
    /// - When no split is found, `Vec` contains all resolved steps (lifecycle
    ///   collected by the caller).
    ///
    /// Centralised here so both [`build_managed_route`] and
    /// [`compile_route_impl`] share the same split-or-resolve logic.
    pub(crate) fn detect_and_validate_route_split(
        &self,
        steps: Vec<BuilderStep>,
        producer_ctx: &ProducerContext,
        route_id: &str,
        staging_mode: &super::step_resolution::FunctionStagingMode,
    ) -> Result<(Option<AggregateSplitInfo>, Vec<CompiledStep>), CamelError> {
        // ── Aggregate split (existing) ──
        if let Some((idx, agg_config)) = find_top_level_aggregate_requiring_split(&steps) {
            let mut pre_steps = steps;
            let mut rest = pre_steps.split_off(idx);
            let _agg_step = rest.remove(0); // invariant: idx points to Aggregate variant
            let post_steps = rest;

            let pre_pairs = self.resolve_steps(
                pre_steps,
                producer_ctx,
                self.registry,
                Some(route_id),
                staging_mode,
            )?;
            let pre_pipeline = super::pipeline_runtime::new_shared_pipeline(compose_pipeline(
                pre_pairs,
                build_pipeline_ctx(self.tracer_metrics, route_id),
            ));

            let post_pairs = self.resolve_steps(
                post_steps,
                producer_ctx,
                self.registry,
                Some(route_id),
                staging_mode,
            )?;
            let post_pipeline = super::pipeline_runtime::new_shared_pipeline(compose_pipeline(
                post_pairs,
                build_pipeline_ctx(self.tracer_metrics, route_id),
            ));

            return Ok((
                Some(AggregateSplitInfo {
                    pre_pipeline,
                    agg_config,
                    post_pipeline,
                }),
                vec![],
            ));
        }

        // ── Resequencer split (Phase 3) ──
        if let Some(reseq_info) = find_top_level_resequencer_requiring_split(&steps)? {
            let idx = reseq_info.index;
            let mut pre_steps = steps;
            let mut rest = pre_steps.split_off(idx);
            let _reseq_step = rest.remove(0); // invariant: idx points to Resequence variant
            let post_steps = rest;

            // Compile pre-steps normally
            let pre_compiled = self.resolve_steps(
                pre_steps,
                producer_ctx,
                self.registry,
                Some(route_id),
                staging_mode,
            )?;

            // Compile post-steps into a BoxProcessor continuation
            let post_compiled = self.resolve_steps(
                post_steps,
                producer_ctx,
                self.registry,
                Some(route_id),
                staging_mode,
            )?;

            // Phase 3 minimal: reject lifecycle-bearing post steps
            if post_compiled.iter().any(|s| match s {
                CompiledStep::Process { lifecycle, .. } => lifecycle.is_some(),
                CompiledStep::Segment { lifecycle, .. } => lifecycle.is_some(),
                CompiledStep::Stop => false,
            }) {
                return Err(CamelError::RouteError(
                    "lifecycle-bearing steps after resequencer are not supported".into(),
                ));
            }

            let post_continuation = compose_pipeline_with_contracts(
                post_compiled,
                None,
                build_pipeline_ctx(self.tracer_metrics, route_id),
            );

            // Create the resequencer policy from the config
            let policy: Arc<dyn ResequencePolicy> = match &reseq_info.policy_config.mode {
                camel_api::ResequenceMode::Batch {
                    correlation,
                    sort,
                    completion,
                } => {
                    // Compile the correlation expression (Simple language)
                    let correlation_def = camel_api::declarative::LanguageExpressionDef {
                        language: "simple".to_string(),
                        source: correlation.clone(),
                    };
                    let correlation_expr = super::step_resolution::compile_language_expression(
                        self.languages,
                        &correlation_def,
                    )?;

                    // Compile the sort expression (Simple language)
                    let sort_def = camel_api::declarative::LanguageExpressionDef {
                        language: "simple".to_string(),
                        source: sort.clone(),
                    };
                    let sort_expr = super::step_resolution::compile_language_expression(
                        self.languages,
                        &sort_def,
                    )?;

                    let batch_policy =
                        BatchPolicy::new_cyclic(correlation_expr, sort_expr, completion.clone());
                    batch_policy as Arc<dyn ResequencePolicy>
                }
                camel_api::ResequenceMode::Stream {
                    sequence,
                    capacity,
                    gap_timeout,
                    on_gap,
                    on_capacity_exceeded,
                    dedup,
                } => {
                    // Compile the sequence expression (Simple language)
                    let seq_def = camel_api::declarative::LanguageExpressionDef {
                        language: "simple".to_string(),
                        source: sequence.clone(),
                    };
                    let sequence_expr = super::step_resolution::compile_language_expression(
                        self.languages,
                        &seq_def,
                    )?;

                    let stream_policy = StreamPolicy::new_cyclic(
                        sequence_expr,
                        *capacity,
                        *gap_timeout,
                        *on_gap,
                        *on_capacity_exceeded,
                        *dedup,
                    );
                    stream_policy as Arc<dyn ResequencePolicy>
                }
            };

            let reseq_config = camel_processor::resequencer::ResequencerConfig {
                metrics: self.tracer_metrics.clone(),
                route_id: Some(route_id.to_string()),
                ..Default::default()
            };
            let resequencer_svc = ResequencerService::with_config(
                policy.clone(),
                post_continuation,
                1024,
                vec![],
                reseq_config,
            );
            let resequencer_lifecycle: Arc<dyn StepLifecycle> = Arc::new(resequencer_svc.clone());

            // Main pipeline: pre + resequencer (last step)
            let mut all_steps = pre_compiled;
            all_steps.push(CompiledStep::Process {
                processor: BoxProcessor::new(resequencer_svc),
                body_contract: None,
                lifecycle: Some(resequencer_lifecycle),
            });

            return Ok((None, all_steps));
        }

        // ── No split — resolve normally ──
        let resolved = self.resolve_steps(
            steps,
            producer_ctx,
            self.registry,
            Some(route_id),
            staging_mode,
        )?;
        Ok((None, resolved))
    }

    /// Shared implementation body for route compilation.
    /// The only difference between `compile_route_definition` and
    /// `compile_route_definition_with_generation` is the `FunctionStagingMode`.
    ///
    /// Returns a [`CompiledPipeline`] so callers (especially the hot-reload
    /// Restart path) can thread lifecycle handles into
    /// [`swap_pipeline_raw`](super::pipeline_runtime::swap_pipeline_raw).
    fn compile_route_impl(
        &self,
        def: RouteDefinition,
        staging_mode: &super::step_resolution::FunctionStagingMode,
    ) -> Result<super::route_helpers::CompiledPipeline, CamelError> {
        let route_id = def.route_id().to_string();

        let producer_ctx = self.build_producer_context(&route_id)?;

        let (aggregate_split, processors_with_contracts) = self.detect_and_validate_route_split(
            def.steps,
            &producer_ctx,
            &route_id,
            staging_mode,
        )?;

        // Aggregate-split routes cannot be fully represented in a
        // `CompiledPipeline` (the split info requires a `ManagedRoute` to
        // store).  Reject them at compile time.
        if aggregate_split.is_some() {
            return Err(CamelError::RouteError(format!(
                "Route '{}' contains an aggregate split that is not supported via the compile-only path; use add_route instead",
                route_id,
            )));
        }

        let lifecycle = super::route_helpers::collect_lifecycle(&processors_with_contracts);

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

        Ok(super::route_helpers::CompiledPipeline {
            processor: pipeline,
            lifecycle,
        })
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
        .map(|c| c.processor)
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
        .map(|c| c.processor)
    }

    /// Compile a route definition with a specific generation and return the
    /// full [`CompiledPipeline`] (processor + lifecycle handles).  Used by
    /// [`apply_swap`](crate::hot_reload::application::reload_actions::apply_swap)
    /// so that the Restart path can thread lifecycle into
    /// [`swap_pipeline_raw`](super::pipeline_runtime::swap_pipeline_raw).
    pub(crate) fn compile_route_definition_pipeline(
        &self,
        def: RouteDefinition,
        generation: u64,
    ) -> Result<super::route_helpers::CompiledPipeline, CamelError> {
        self.compile_route_impl(
            def,
            &super::step_resolution::FunctionStagingMode::HotReload { generation },
        )
    }

    /// Compile a route definition without function generation and return the
    /// full [`CompiledPipeline`] (processor + lifecycle handles).
    ///
    /// Oracle Fix 1: used by the stateless hot-reload path (`function_ctx
    /// == None`) so that lifecycle-bearing routes (resequencer, aggregator)
    /// have their handles preserved and properly drained on future stops.
    pub(crate) fn compile_route_definition_dry_pipeline(
        &self,
        def: RouteDefinition,
    ) -> Result<super::route_helpers::CompiledPipeline, CamelError> {
        self.compile_route_impl(
            def,
            &super::step_resolution::FunctionStagingMode::DryCompile,
        )
    }
}
