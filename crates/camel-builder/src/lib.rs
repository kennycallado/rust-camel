use camel_api::aggregator::{
    AggregationStrategy, AggregatorConfig, CompletionCondition, CompletionMode, CorrelationStrategy,
};
use camel_api::body::Body;
use camel_api::body_converter::BodyType;
use camel_api::circuit_breaker::CircuitBreakerConfig;
use camel_api::dynamic_router::{DynamicRouterConfig, RouterExpression};
use camel_api::error_handler::{ErrorHandlerConfig, RedeliveryPolicy};
use camel_api::load_balancer::LoadBalancerConfig;
use camel_api::multicast::{MulticastConfig, MulticastStrategy};
use camel_api::routing_slip::{RoutingSlipConfig, RoutingSlipExpression};
use camel_api::splitter::SplitterConfig;
use camel_api::throttler::{ThrottleStrategy, ThrottlerConfig};
use camel_api::DelayConfig;
use camel_api::{
    BoxProcessor, CamelError, CanonicalRouteSpec, Exchange, FilterPredicate, IdentityProcessor,
    ProcessorFn, Value,
    runtime::{
        CanonicalAggregateSpec, CanonicalAggregateStrategySpec, CanonicalCircuitBreakerSpec,
        CanonicalSplitAggregationSpec, CanonicalSplitExpressionSpec, CanonicalStepSpec,
        CanonicalWhenSpec,
    },
};
use camel_component_api::ConcurrencyModel;
use camel_core::route::{BuilderStep, DeclarativeWhenStep, RouteDefinition, WhenStep};
use camel_processor::{
    ConvertBodyTo, DynamicSetHeader, LogLevel, MapBody, MarshalService, SetBody, SetHeader,
    UnmarshalService, builtin_data_format,
};

/// Shared step-accumulation methods for all builder types.
///
/// Implementors provide `steps_mut()` and get step-adding methods for free.
/// `filter()` and other branching methods are NOT included — they return
/// different types per builder and stay as per-builder methods.
pub trait StepAccumulator: Sized {
    fn steps_mut(&mut self) -> &mut Vec<BuilderStep>;

    fn to(mut self, endpoint: impl Into<String>) -> Self {
        self.steps_mut().push(BuilderStep::To(endpoint.into()));
        self
    }

    fn process<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(Exchange) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Exchange, CamelError>> + Send + 'static,
    {
        let svc = ProcessorFn::new(f);
        self.steps_mut()
            .push(BuilderStep::Processor(BoxProcessor::new(svc)));
        self
    }

    fn process_fn(mut self, processor: BoxProcessor) -> Self {
        self.steps_mut().push(BuilderStep::Processor(processor));
        self
    }

    fn set_header(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        let svc = SetHeader::new(IdentityProcessor, key, value);
        self.steps_mut()
            .push(BuilderStep::Processor(BoxProcessor::new(svc)));
        self
    }

    fn map_body<F>(mut self, mapper: F) -> Self
    where
        F: Fn(Body) -> Body + Clone + Send + Sync + 'static,
    {
        let svc = MapBody::new(IdentityProcessor, mapper);
        self.steps_mut()
            .push(BuilderStep::Processor(BoxProcessor::new(svc)));
        self
    }

    fn set_body<B>(mut self, body: B) -> Self
    where
        B: Into<Body> + Clone + Send + Sync + 'static,
    {
        let body: Body = body.into();
        let svc = SetBody::new(IdentityProcessor, move |_ex: &Exchange| body.clone());
        self.steps_mut()
            .push(BuilderStep::Processor(BoxProcessor::new(svc)));
        self
    }

    /// Apache Camel-compatible alias for [`set_body`](Self::set_body).
    ///
    /// Transforms the message body using the given value. Semantically identical
    /// to `set_body` — provided for familiarity with Apache Camel route DSLs.
    fn transform<B>(self, body: B) -> Self
    where
        B: Into<Body> + Clone + Send + Sync + 'static,
    {
        self.set_body(body)
    }

    fn set_body_fn<F>(mut self, expr: F) -> Self
    where
        F: Fn(&Exchange) -> Body + Clone + Send + Sync + 'static,
    {
        let svc = SetBody::new(IdentityProcessor, expr);
        self.steps_mut()
            .push(BuilderStep::Processor(BoxProcessor::new(svc)));
        self
    }

    fn set_header_fn<F>(mut self, key: impl Into<String>, expr: F) -> Self
    where
        F: Fn(&Exchange) -> Value + Clone + Send + Sync + 'static,
    {
        let svc = DynamicSetHeader::new(IdentityProcessor, key, expr);
        self.steps_mut()
            .push(BuilderStep::Processor(BoxProcessor::new(svc)));
        self
    }

    fn aggregate(mut self, config: AggregatorConfig) -> Self {
        self.steps_mut().push(BuilderStep::Aggregate { config });
        self
    }

    /// Stop processing this exchange immediately. No further steps in the
    /// current pipeline will run.
    ///
    /// Can be used at any point in the route: directly on RouteBuilder,
    /// inside `.filter()`, inside `.split()`, etc.
    fn stop(mut self) -> Self {
        self.steps_mut().push(BuilderStep::Stop);
        self
    }

    fn delay(mut self, duration: std::time::Duration) -> Self {
        self.steps_mut().push(BuilderStep::Delay {
            config: DelayConfig::from_duration(duration),
        });
        self
    }

    fn delay_with_header(
        mut self,
        duration: std::time::Duration,
        header: impl Into<String>,
    ) -> Self {
        self.steps_mut().push(BuilderStep::Delay {
            config: DelayConfig::from_duration_with_header(duration, header),
        });
        self
    }

    /// Log a message at the specified level.
    ///
    /// The message will be logged when an exchange passes through this step.
    fn log(mut self, message: impl Into<String>, level: LogLevel) -> Self {
        self.steps_mut().push(BuilderStep::Log {
            level,
            message: message.into(),
        });
        self
    }

    /// Convert the message body to the target type.
    ///
    /// Supported: Text ↔ Json ↔ Bytes. `Body::Stream` always fails.
    /// Returns `TypeConversionFailed` if conversion is not possible.
    ///
    /// # Example
    /// ```ignore
    /// route.set_body(Value::String(r#"{"x":1}"#.into()))
    ///      .convert_body_to(BodyType::Json)
    ///      .to("direct:next")
    /// ```
    fn convert_body_to(mut self, target: BodyType) -> Self {
        let svc = ConvertBodyTo::new(IdentityProcessor, target);
        self.steps_mut()
            .push(BuilderStep::Processor(BoxProcessor::new(svc)));
        self
    }

    /// Marshal the message body using the specified data format.
    ///
    /// Supported formats: `"json"`, `"xml"`. Panics if the format name is unknown.
    /// Converts a structured body (e.g., `Body::Json`) to a wire-format body (e.g., `Body::Text`).
    ///
    /// # Example
    /// ```ignore
    /// route.marshal("json").to("direct:next")
    /// ```
    fn marshal(mut self, format: impl Into<String>) -> Self {
        let name = format.into();
        let df =
            builtin_data_format(&name).unwrap_or_else(|| panic!("unknown data format: '{name}'"));
        let svc = MarshalService::new(IdentityProcessor, df);
        self.steps_mut()
            .push(BuilderStep::Processor(BoxProcessor::new(svc)));
        self
    }

    /// Unmarshal the message body using the specified data format.
    ///
    /// Supported formats: `"json"`, `"xml"`. Panics if the format name is unknown.
    /// Converts a wire-format body (e.g., `Body::Text`) to a structured body (e.g., `Body::Json`).
    ///
    /// # Example
    /// ```ignore
    /// route.unmarshal("json").to("direct:next")
    /// ```
    fn unmarshal(mut self, format: impl Into<String>) -> Self {
        let name = format.into();
        let df =
            builtin_data_format(&name).unwrap_or_else(|| panic!("unknown data format: '{name}'"));
        let svc = UnmarshalService::new(IdentityProcessor, df);
        self.steps_mut()
            .push(BuilderStep::Processor(BoxProcessor::new(svc)));
        self
    }

    /// Execute a script that can modify the exchange (headers, properties, body).
    ///
    /// The script has access to `headers`, `properties`, and `body` variables
    /// and can modify them with assignment syntax: `headers["k"] = v`.
    ///
    /// # Example
    /// ```ignore
    /// // ignore: requires full CamelContext setup with registered language
    /// route.script("rhai", r#"headers["tenant"] = "acme"; body = body + "_processed""#)
    /// ```
    fn script(mut self, language: impl Into<String>, script: impl Into<String>) -> Self {
        self.steps_mut().push(BuilderStep::Script {
            language: language.into(),
            script: script.into(),
        });
        self
    }
}

/// A fluent builder for constructing routes.
///
/// # Example
///
/// ```ignore
/// let definition = RouteBuilder::from("timer:tick?period=1000")
///     .set_header("source", Value::String("timer".into()))
///     .filter(|ex| ex.input.body.as_text().is_some())
///     .to("log:info?showHeaders=true")
///     .build()?;
/// ```
pub struct RouteBuilder {
    from_uri: String,
    steps: Vec<BuilderStep>,
    error_handler: Option<ErrorHandlerConfig>,
    error_handler_mode: ErrorHandlerMode,
    circuit_breaker_config: Option<CircuitBreakerConfig>,
    concurrency: Option<ConcurrencyModel>,
    route_id: Option<String>,
    auto_startup: Option<bool>,
    startup_order: Option<i32>,
}

#[derive(Default)]
enum ErrorHandlerMode {
    #[default]
    None,
    ExplicitConfig,
    Shorthand {
        dlc_uri: Option<String>,
        specs: Vec<OnExceptionSpec>,
    },
    Mixed,
}

#[derive(Clone)]
struct OnExceptionSpec {
    matches: std::sync::Arc<dyn Fn(&CamelError) -> bool + Send + Sync>,
    retry: Option<RedeliveryPolicy>,
    handled_by: Option<String>,
}

impl RouteBuilder {
    /// Start building a route from the given source endpoint URI.
    pub fn from(endpoint: &str) -> Self {
        Self {
            from_uri: endpoint.to_string(),
            steps: Vec::new(),
            error_handler: None,
            error_handler_mode: ErrorHandlerMode::None,
            circuit_breaker_config: None,
            concurrency: None,
            route_id: None,
            auto_startup: None,
            startup_order: None,
        }
    }

    /// Open a filter scope. Only exchanges matching `predicate` will be processed
    /// by the steps inside the scope. Non-matching exchanges skip the scope entirely
    /// and continue to steps after `.end_filter()`.
    pub fn filter<F>(self, predicate: F) -> FilterBuilder
    where
        F: Fn(&Exchange) -> bool + Send + Sync + 'static,
    {
        FilterBuilder {
            parent: self,
            predicate: std::sync::Arc::new(predicate),
            steps: vec![],
        }
    }

    /// Open a choice scope for content-based routing.
    ///
    /// Within the choice, you can define multiple `.when()` clauses and an
    /// optional `.otherwise()` clause. The first matching `when` predicate
    /// determines which sub-pipeline executes.
    pub fn choice(self) -> ChoiceBuilder {
        ChoiceBuilder {
            parent: self,
            whens: vec![],
            _otherwise: None,
        }
    }

    /// Add a WireTap step that sends a clone of the exchange to the given
    /// endpoint URI (fire-and-forget). The original exchange continues
    /// downstream unchanged.
    pub fn wire_tap(mut self, endpoint: &str) -> Self {
        self.steps.push(BuilderStep::WireTap {
            uri: endpoint.to_string(),
        });
        self
    }

    /// Set a per-route error handler. Overrides the global error handler on `CamelContext`.
    pub fn error_handler(mut self, config: ErrorHandlerConfig) -> Self {
        self.error_handler_mode = match self.error_handler_mode {
            ErrorHandlerMode::None | ErrorHandlerMode::ExplicitConfig => {
                ErrorHandlerMode::ExplicitConfig
            }
            ErrorHandlerMode::Shorthand { .. } | ErrorHandlerMode::Mixed => ErrorHandlerMode::Mixed,
        };
        self.error_handler = Some(config);
        self
    }

    /// Set a dead letter channel URI for shorthand error handler mode.
    pub fn dead_letter_channel(mut self, uri: impl Into<String>) -> Self {
        let uri = uri.into();
        self.error_handler_mode = match self.error_handler_mode {
            ErrorHandlerMode::None => ErrorHandlerMode::Shorthand {
                dlc_uri: Some(uri),
                specs: Vec::new(),
            },
            ErrorHandlerMode::Shorthand { specs, .. } => ErrorHandlerMode::Shorthand {
                dlc_uri: Some(uri),
                specs,
            },
            ErrorHandlerMode::ExplicitConfig | ErrorHandlerMode::Mixed => ErrorHandlerMode::Mixed,
        };
        self
    }

    /// Add a shorthand exception policy scope. Call `.end_on_exception()` to return to route builder.
    pub fn on_exception<F>(mut self, matches: F) -> OnExceptionBuilder
    where
        F: Fn(&CamelError) -> bool + Send + Sync + 'static,
    {
        self.error_handler_mode = match self.error_handler_mode {
            ErrorHandlerMode::None => ErrorHandlerMode::Shorthand {
                dlc_uri: None,
                specs: Vec::new(),
            },
            ErrorHandlerMode::ExplicitConfig | ErrorHandlerMode::Mixed => ErrorHandlerMode::Mixed,
            shorthand @ ErrorHandlerMode::Shorthand { .. } => shorthand,
        };

        OnExceptionBuilder {
            parent: self,
            policy: OnExceptionSpec {
                matches: std::sync::Arc::new(matches),
                retry: None,
                handled_by: None,
            },
        }
    }

    /// Set a circuit breaker for this route.
    pub fn circuit_breaker(mut self, config: CircuitBreakerConfig) -> Self {
        self.circuit_breaker_config = Some(config);
        self
    }

    /// Override the consumer's default concurrency model.
    ///
    /// When set, the pipeline spawns a task per exchange, processing them
    /// concurrently. `max` limits the number of simultaneously active
    /// pipeline executions (0 = unbounded, channel buffer is backpressure).
    ///
    /// # Example
    /// ```ignore
    /// RouteBuilder::from("http://0.0.0.0:8080/api")
    ///     .concurrent(16)  // max 16 in-flight pipeline executions
    ///     .process(handle_request)
    ///     .build()
    /// ```
    pub fn concurrent(mut self, max: usize) -> Self {
        let max = if max == 0 { None } else { Some(max) };
        self.concurrency = Some(ConcurrencyModel::Concurrent { max });
        self
    }

    /// Force sequential processing, overriding a concurrent-capable consumer.
    ///
    /// Useful for HTTP routes that mutate shared state and need ordering
    /// guarantees.
    pub fn sequential(mut self) -> Self {
        self.concurrency = Some(ConcurrencyModel::Sequential);
        self
    }

    /// Set the route ID for this route.
    ///
    /// If not set, the route will be assigned an auto-generated ID.
    pub fn route_id(mut self, id: impl Into<String>) -> Self {
        self.route_id = Some(id.into());
        self
    }

    /// Set whether this route should automatically start when the context starts.
    ///
    /// Default is `true`.
    pub fn auto_startup(mut self, auto: bool) -> Self {
        self.auto_startup = Some(auto);
        self
    }

    /// Set the startup order for this route.
    ///
    /// Routes with lower values start first. Default is 1000.
    pub fn startup_order(mut self, order: i32) -> Self {
        self.startup_order = Some(order);
        self
    }

    /// Begin a Splitter sub-pipeline. Steps added after this call (until
    /// `.end_split()`) will be executed per-fragment.
    ///
    /// Returns a `SplitBuilder` — you cannot call `.build()` until
    /// `.end_split()` closes the split scope (enforced by the type system).
    pub fn split(self, config: SplitterConfig) -> SplitBuilder {
        SplitBuilder {
            parent: self,
            config,
            steps: Vec::new(),
        }
    }

    /// Begin a Multicast sub-pipeline. Steps added after this call (until
    /// `.end_multicast()`) will each receive a copy of the exchange.
    ///
    /// Returns a `MulticastBuilder` — you cannot call `.build()` until
    /// `.end_multicast()` closes the multicast scope (enforced by the type system).
    pub fn multicast(self) -> MulticastBuilder {
        MulticastBuilder {
            parent: self,
            steps: Vec::new(),
            config: MulticastConfig::new(),
        }
    }

    /// Begin a Throttle sub-pipeline. Rate limits message processing to at most
    /// `max_requests` per `period`. Steps inside the throttle scope are only
    /// executed when the rate limit allows.
    ///
    /// Returns a `ThrottleBuilder` — you cannot call `.build()` until
    /// `.end_throttle()` closes the throttle scope (enforced by the type system).
    pub fn throttle(self, max_requests: usize, period: std::time::Duration) -> ThrottleBuilder {
        ThrottleBuilder {
            parent: self,
            config: ThrottlerConfig::new(max_requests, period),
            steps: Vec::new(),
        }
    }

    /// Begin a LoadBalance sub-pipeline. Distributes exchanges across multiple
    /// endpoints using a configurable strategy (round-robin, random, weighted, failover).
    ///
    /// Returns a `LoadBalancerBuilder` — you cannot call `.build()` until
    /// `.end_load_balance()` closes the load balance scope (enforced by the type system).
    pub fn load_balance(self) -> LoadBalancerBuilder {
        LoadBalancerBuilder {
            parent: self,
            config: LoadBalancerConfig::round_robin(),
            steps: Vec::new(),
        }
    }

    /// Add a dynamic router step that routes exchanges dynamically based on
    /// expression evaluation at runtime.
    ///
    /// The expression receives the exchange and returns `Some(uri)` to route to
    /// the next endpoint, or `None` to stop routing.
    ///
    /// # Example
    /// ```ignore
    /// RouteBuilder::from("timer:tick")
    ///     .route_id("test-route")
    ///     .dynamic_router(|ex| {
    ///         ex.input.header("dest").and_then(|v| v.as_str().map(|s| s.to_string()))
    ///     })
    ///     .build()
    /// ```
    pub fn dynamic_router(self, expression: RouterExpression) -> Self {
        self.dynamic_router_with_config(DynamicRouterConfig::new(expression))
    }

    /// Add a dynamic router step with full configuration.
    ///
    /// Allows customization of URI delimiter, cache size, timeout, and other options.
    pub fn dynamic_router_with_config(mut self, config: DynamicRouterConfig) -> Self {
        self.steps.push(BuilderStep::DynamicRouter { config });
        self
    }

    pub fn routing_slip(self, expression: RoutingSlipExpression) -> Self {
        self.routing_slip_with_config(RoutingSlipConfig::new(expression))
    }

    pub fn routing_slip_with_config(mut self, config: RoutingSlipConfig) -> Self {
        self.steps.push(BuilderStep::RoutingSlip { config });
        self
    }

    /// Consume the builder and produce a [`RouteDefinition`].
    pub fn build(self) -> Result<RouteDefinition, CamelError> {
        if self.from_uri.is_empty() {
            return Err(CamelError::RouteError(
                "route must have a 'from' URI".to_string(),
            ));
        }
        let route_id = self.route_id.ok_or_else(|| {
            CamelError::RouteError(
                "route must have a 'route_id' — call .route_id(\"name\") on the builder"
                    .to_string(),
            )
        })?;
        let resolved_error_handler = match self.error_handler_mode {
            ErrorHandlerMode::None => self.error_handler,
            ErrorHandlerMode::ExplicitConfig => self.error_handler,
            ErrorHandlerMode::Mixed => {
                return Err(CamelError::RouteError(
                    "mixed error handler modes: cannot combine .error_handler(config) with shorthand methods".into(),
                ));
            }
            ErrorHandlerMode::Shorthand { dlc_uri, specs } => {
                let mut config = if let Some(uri) = dlc_uri {
                    ErrorHandlerConfig::dead_letter_channel(uri)
                } else {
                    ErrorHandlerConfig::log_only()
                };

                for spec in specs {
                    let matcher = spec.matches.clone();
                    let mut builder = config.on_exception(move |e| matcher(e));

                    if let Some(retry) = spec.retry {
                        builder = builder.retry(retry.max_attempts).with_backoff(
                            retry.initial_delay,
                            retry.multiplier,
                            retry.max_delay,
                        );
                        if retry.jitter_factor > 0.0 {
                            builder = builder.with_jitter(retry.jitter_factor);
                        }
                    }

                    if let Some(uri) = spec.handled_by {
                        builder = builder.handled_by(uri);
                    }

                    config = builder.build();
                }

                Some(config)
            }
        };

        let definition = RouteDefinition::new(self.from_uri, self.steps);
        let definition = if let Some(eh) = resolved_error_handler {
            definition.with_error_handler(eh)
        } else {
            definition
        };
        let definition = if let Some(cb) = self.circuit_breaker_config {
            definition.with_circuit_breaker(cb)
        } else {
            definition
        };
        let definition = if let Some(concurrency) = self.concurrency {
            definition.with_concurrency(concurrency)
        } else {
            definition
        };
        let definition = definition.with_route_id(route_id);
        let definition = if let Some(auto) = self.auto_startup {
            definition.with_auto_startup(auto)
        } else {
            definition
        };
        let definition = if let Some(order) = self.startup_order {
            definition.with_startup_order(order)
        } else {
            definition
        };
        Ok(definition)
    }

    /// Compile this builder route into canonical v1 spec.
    pub fn build_canonical(self) -> Result<CanonicalRouteSpec, CamelError> {
        if self.from_uri.is_empty() {
            return Err(CamelError::RouteError(
                "route must have a 'from' URI".to_string(),
            ));
        }
        let route_id = self.route_id.ok_or_else(|| {
            CamelError::RouteError(
                "route must have a 'route_id' — call .route_id(\"name\") on the builder"
                    .to_string(),
            )
        })?;

        let steps = canonicalize_steps(self.steps)?;
        let circuit_breaker = self
            .circuit_breaker_config
            .map(canonicalize_circuit_breaker);

        let spec = CanonicalRouteSpec {
            route_id,
            from: self.from_uri,
            steps,
            circuit_breaker,
            version: camel_api::CANONICAL_CONTRACT_VERSION,
        };
        spec.validate_contract()?;
        Ok(spec)
    }
}

pub struct OnExceptionBuilder {
    parent: RouteBuilder,
    policy: OnExceptionSpec,
}

impl OnExceptionBuilder {
    pub fn retry(mut self, max_attempts: u32) -> Self {
        self.policy.retry = Some(RedeliveryPolicy::new(max_attempts));
        self
    }

    pub fn with_backoff(
        mut self,
        initial: std::time::Duration,
        multiplier: f64,
        max: std::time::Duration,
    ) -> Self {
        if let Some(ref mut retry) = self.policy.retry {
            retry.initial_delay = initial;
            retry.multiplier = multiplier;
            retry.max_delay = max;
        }
        self
    }

    pub fn with_jitter(mut self, jitter_factor: f64) -> Self {
        if let Some(ref mut retry) = self.policy.retry {
            retry.jitter_factor = jitter_factor.clamp(0.0, 1.0);
        }
        self
    }

    pub fn handled_by(mut self, uri: impl Into<String>) -> Self {
        self.policy.handled_by = Some(uri.into());
        self
    }

    pub fn end_on_exception(mut self) -> RouteBuilder {
        if let ErrorHandlerMode::Shorthand { ref mut specs, .. } = self.parent.error_handler_mode {
            specs.push(self.policy);
        }
        self.parent
    }
}

fn canonicalize_steps(steps: Vec<BuilderStep>) -> Result<Vec<CanonicalStepSpec>, CamelError> {
    let mut canonical = Vec::with_capacity(steps.len());
    for step in steps {
        canonical.push(canonicalize_step(step)?);
    }
    Ok(canonical)
}

fn canonicalize_step(step: BuilderStep) -> Result<CanonicalStepSpec, CamelError> {
    match step {
        BuilderStep::To(uri) => Ok(CanonicalStepSpec::To { uri }),
        BuilderStep::Log { message, .. } => Ok(CanonicalStepSpec::Log { message }),
        BuilderStep::Stop => Ok(CanonicalStepSpec::Stop),
        BuilderStep::WireTap { uri } => Ok(CanonicalStepSpec::WireTap { uri }),
        BuilderStep::Delay { config } => Ok(CanonicalStepSpec::Delay {
            delay_ms: config.delay_ms,
            dynamic_header: config.dynamic_header,
        }),
        BuilderStep::DeclarativeScript { expression } => {
            Ok(CanonicalStepSpec::Script { expression })
        }
        BuilderStep::DeclarativeFilter { predicate, steps } => Ok(CanonicalStepSpec::Filter {
            predicate,
            steps: canonicalize_steps(steps)?,
        }),
        BuilderStep::DeclarativeChoice { whens, otherwise } => {
            let mut canonical_whens = Vec::with_capacity(whens.len());
            for DeclarativeWhenStep { predicate, steps } in whens {
                canonical_whens.push(CanonicalWhenSpec {
                    predicate,
                    steps: canonicalize_steps(steps)?,
                });
            }
            let otherwise = match otherwise {
                Some(steps) => Some(canonicalize_steps(steps)?),
                None => None,
            };
            Ok(CanonicalStepSpec::Choice {
                whens: canonical_whens,
                otherwise,
            })
        }
        BuilderStep::DeclarativeSplit {
            expression,
            aggregation,
            parallel,
            parallel_limit,
            stop_on_exception,
            steps,
        } => Ok(CanonicalStepSpec::Split {
            expression: CanonicalSplitExpressionSpec::Language(expression),
            aggregation: canonicalize_split_aggregation(aggregation)?,
            parallel,
            parallel_limit,
            stop_on_exception,
            steps: canonicalize_steps(steps)?,
        }),
        BuilderStep::Aggregate { config } => Ok(CanonicalStepSpec::Aggregate {
            config: canonicalize_aggregate(config)?,
        }),
        other => {
            let step_name = canonical_step_name(&other);
            let detail = camel_api::canonical_contract_rejection_reason(step_name)
                .unwrap_or("not included in canonical v1");
            Err(CamelError::RouteError(format!(
                "canonical v1 does not support step `{step_name}`: {detail}"
            )))
        }
    }
}

fn canonicalize_split_aggregation(
    strategy: camel_api::splitter::AggregationStrategy,
) -> Result<CanonicalSplitAggregationSpec, CamelError> {
    match strategy {
        camel_api::splitter::AggregationStrategy::LastWins => {
            Ok(CanonicalSplitAggregationSpec::LastWins)
        }
        camel_api::splitter::AggregationStrategy::CollectAll => {
            Ok(CanonicalSplitAggregationSpec::CollectAll)
        }
        camel_api::splitter::AggregationStrategy::Custom(_) => Err(CamelError::RouteError(
            "canonical v1 does not support custom split aggregation".to_string(),
        )),
        camel_api::splitter::AggregationStrategy::Original => {
            Ok(CanonicalSplitAggregationSpec::Original)
        }
    }
}

fn extract_completion_fields(
    mode: &CompletionMode,
) -> Result<(Option<usize>, Option<u64>), CamelError> {
    match mode {
        CompletionMode::Single(cond) => match cond {
            CompletionCondition::Size(n) => Ok((Some(*n), None)),
            CompletionCondition::Timeout(d) => Ok((None, Some(d.as_millis() as u64))),
            CompletionCondition::Predicate(_) => Err(CamelError::RouteError(
                "canonical v1 does not support aggregate predicate completion".to_string(),
            )),
        },
        CompletionMode::Any(conds) => {
            let mut size = None;
            let mut timeout_ms = None;
            for cond in conds {
                match cond {
                    CompletionCondition::Size(n) => size = Some(*n),
                    CompletionCondition::Timeout(d) => timeout_ms = Some(d.as_millis() as u64),
                    CompletionCondition::Predicate(_) => {
                        return Err(CamelError::RouteError(
                            "canonical v1 does not support aggregate predicate completion"
                                .to_string(),
                        ));
                    }
                }
            }
            Ok((size, timeout_ms))
        }
    }
}

fn canonicalize_aggregate(config: AggregatorConfig) -> Result<CanonicalAggregateSpec, CamelError> {
    let (completion_size, completion_timeout_ms) = extract_completion_fields(&config.completion)?;

    let header = match &config.correlation {
        CorrelationStrategy::HeaderName(h) => h.clone(),
        CorrelationStrategy::Expression { expr, .. } => expr.clone(),
        CorrelationStrategy::Fn(_) => {
            return Err(CamelError::RouteError(
                "canonical v1 does not support Fn correlation strategy".to_string(),
            ));
        }
    };

    let correlation_key = match &config.correlation {
        CorrelationStrategy::HeaderName(_) => None,
        CorrelationStrategy::Expression { expr, .. } => Some(expr.clone()),
        CorrelationStrategy::Fn(_) => unreachable!(),
    };

    let strategy = match config.strategy {
        AggregationStrategy::CollectAll => CanonicalAggregateStrategySpec::CollectAll,
        AggregationStrategy::Custom(_) => {
            return Err(CamelError::RouteError(
                "canonical v1 does not support custom aggregate strategy".to_string(),
            ));
        }
    };
    let bucket_ttl_ms = config
        .bucket_ttl
        .map(|ttl| u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX));

    Ok(CanonicalAggregateSpec {
        header,
        completion_size,
        completion_timeout_ms,
        correlation_key,
        force_completion_on_stop: if config.force_completion_on_stop {
            Some(true)
        } else {
            None
        },
        discard_on_timeout: if config.discard_on_timeout {
            Some(true)
        } else {
            None
        },
        strategy,
        max_buckets: config.max_buckets,
        bucket_ttl_ms,
    })
}

fn canonicalize_circuit_breaker(config: CircuitBreakerConfig) -> CanonicalCircuitBreakerSpec {
    CanonicalCircuitBreakerSpec {
        failure_threshold: config.failure_threshold,
        open_duration_ms: u64::try_from(config.open_duration.as_millis()).unwrap_or(u64::MAX),
    }
}

fn canonical_step_name(step: &BuilderStep) -> &'static str {
    match step {
        BuilderStep::Processor(_) => "processor",
        BuilderStep::To(_) => "to",
        BuilderStep::Stop => "stop",
        BuilderStep::Log { .. } => "log",
        BuilderStep::DeclarativeSetHeader { .. } => "set_header",
        BuilderStep::DeclarativeSetBody { .. } => "set_body",
        BuilderStep::DeclarativeFilter { .. } => "filter",
        BuilderStep::DeclarativeChoice { .. } => "choice",
        BuilderStep::DeclarativeScript { .. } => "script",
        BuilderStep::DeclarativeSplit { .. } => "split",
        BuilderStep::Split { .. } => "split",
        BuilderStep::Aggregate { .. } => "aggregate",
        BuilderStep::Filter { .. } => "filter",
        BuilderStep::Choice { .. } => "choice",
        BuilderStep::WireTap { .. } => "wire_tap",
        BuilderStep::Delay { .. } => "delay",
        BuilderStep::Multicast { .. } => "multicast",
        BuilderStep::DeclarativeLog { .. } => "log",
        BuilderStep::Bean { .. } => "bean",
        BuilderStep::Script { .. } => "script",
        BuilderStep::Throttle { .. } => "throttle",
        BuilderStep::LoadBalance { .. } => "load_balancer",
        BuilderStep::DynamicRouter { .. } => "dynamic_router",
        BuilderStep::RoutingSlip { .. } => "routing_slip",
        BuilderStep::DeclarativeDynamicRouter { .. } => "declarative_dynamic_router",
        BuilderStep::DeclarativeRoutingSlip { .. } => "declarative_routing_slip",
    }
}

impl StepAccumulator for RouteBuilder {
    fn steps_mut(&mut self) -> &mut Vec<BuilderStep> {
        &mut self.steps
    }
}

/// Builder for the sub-pipeline within a `.split()` ... `.end_split()` block.
///
/// Exposes the same step methods as `RouteBuilder` (to, process, filter, etc.)
/// but NOT `.build()` and NOT `.split()` (no nested splits).
///
/// Calling `.end_split()` packages the sub-steps into a `BuilderStep::Split`
/// and returns the parent `RouteBuilder`.
pub struct SplitBuilder {
    parent: RouteBuilder,
    config: SplitterConfig,
    steps: Vec<BuilderStep>,
}

impl SplitBuilder {
    /// Open a filter scope within the split sub-pipeline.
    pub fn filter<F>(self, predicate: F) -> FilterInSplitBuilder
    where
        F: Fn(&Exchange) -> bool + Send + Sync + 'static,
    {
        FilterInSplitBuilder {
            parent: self,
            predicate: std::sync::Arc::new(predicate),
            steps: vec![],
        }
    }

    /// Close the split scope. Packages the accumulated sub-steps into a
    /// `BuilderStep::Split` and returns the parent `RouteBuilder`.
    pub fn end_split(mut self) -> RouteBuilder {
        let split_step = BuilderStep::Split {
            config: self.config,
            steps: self.steps,
        };
        self.parent.steps.push(split_step);
        self.parent
    }
}

impl StepAccumulator for SplitBuilder {
    fn steps_mut(&mut self) -> &mut Vec<BuilderStep> {
        &mut self.steps
    }
}

/// Builder for the sub-pipeline within a `.filter()` ... `.end_filter()` block.
pub struct FilterBuilder {
    parent: RouteBuilder,
    predicate: FilterPredicate,
    steps: Vec<BuilderStep>,
}

impl FilterBuilder {
    /// Close the filter scope. Packages the accumulated sub-steps into a
    /// `BuilderStep::Filter` and returns the parent `RouteBuilder`.
    pub fn end_filter(mut self) -> RouteBuilder {
        let step = BuilderStep::Filter {
            predicate: self.predicate,
            steps: self.steps,
        };
        self.parent.steps.push(step);
        self.parent
    }
}

impl StepAccumulator for FilterBuilder {
    fn steps_mut(&mut self) -> &mut Vec<BuilderStep> {
        &mut self.steps
    }
}

/// Builder for a filter scope nested inside a `.split()` block.
pub struct FilterInSplitBuilder {
    parent: SplitBuilder,
    predicate: FilterPredicate,
    steps: Vec<BuilderStep>,
}

impl FilterInSplitBuilder {
    /// Close the filter scope and return the parent `SplitBuilder`.
    pub fn end_filter(mut self) -> SplitBuilder {
        let step = BuilderStep::Filter {
            predicate: self.predicate,
            steps: self.steps,
        };
        self.parent.steps.push(step);
        self.parent
    }
}

impl StepAccumulator for FilterInSplitBuilder {
    fn steps_mut(&mut self) -> &mut Vec<BuilderStep> {
        &mut self.steps
    }
}

// ── Choice/When/Otherwise builders ─────────────────────────────────────────

/// Builder for a `.choice()` ... `.end_choice()` block.
///
/// Accumulates `when` clauses and an optional `otherwise` clause.
/// Cannot call `.build()` until `.end_choice()` is called.
pub struct ChoiceBuilder {
    parent: RouteBuilder,
    whens: Vec<WhenStep>,
    _otherwise: Option<Vec<BuilderStep>>,
}

impl ChoiceBuilder {
    /// Open a `when` clause. Only exchanges matching `predicate` will be
    /// processed by the steps inside the `.when()` ... `.end_when()` scope.
    pub fn when<F>(self, predicate: F) -> WhenBuilder
    where
        F: Fn(&Exchange) -> bool + Send + Sync + 'static,
    {
        WhenBuilder {
            parent: self,
            predicate: std::sync::Arc::new(predicate),
            steps: vec![],
        }
    }

    /// Open an `otherwise` clause. Executed when no `when` predicate matched.
    ///
    /// Only one `otherwise` is allowed per `choice`. Call this after all `.when()` clauses.
    pub fn otherwise(self) -> OtherwiseBuilder {
        OtherwiseBuilder {
            parent: self,
            steps: vec![],
        }
    }

    /// Close the choice scope. Packages all accumulated `when` clauses and
    /// optional `otherwise` into a `BuilderStep::Choice` and returns the
    /// parent `RouteBuilder`.
    pub fn end_choice(mut self) -> RouteBuilder {
        let step = BuilderStep::Choice {
            whens: self.whens,
            otherwise: self._otherwise,
        };
        self.parent.steps.push(step);
        self.parent
    }
}

/// Builder for the sub-pipeline within a `.when()` ... `.end_when()` block.
pub struct WhenBuilder {
    parent: ChoiceBuilder,
    predicate: camel_api::FilterPredicate,
    steps: Vec<BuilderStep>,
}

impl WhenBuilder {
    /// Close the when scope. Packages the accumulated sub-steps into a
    /// `WhenStep` and returns the parent `ChoiceBuilder`.
    pub fn end_when(mut self) -> ChoiceBuilder {
        self.parent.whens.push(WhenStep {
            predicate: self.predicate,
            steps: self.steps,
        });
        self.parent
    }
}

impl StepAccumulator for WhenBuilder {
    fn steps_mut(&mut self) -> &mut Vec<BuilderStep> {
        &mut self.steps
    }
}

/// Builder for the sub-pipeline within an `.otherwise()` ... `.end_otherwise()` block.
pub struct OtherwiseBuilder {
    parent: ChoiceBuilder,
    steps: Vec<BuilderStep>,
}

impl OtherwiseBuilder {
    /// Close the otherwise scope and return the parent `ChoiceBuilder`.
    pub fn end_otherwise(self) -> ChoiceBuilder {
        let OtherwiseBuilder { mut parent, steps } = self;
        parent._otherwise = Some(steps);
        parent
    }
}

impl StepAccumulator for OtherwiseBuilder {
    fn steps_mut(&mut self) -> &mut Vec<BuilderStep> {
        &mut self.steps
    }
}

/// Builder for the sub-pipeline within a `.multicast()` ... `.end_multicast()` block.
///
/// Exposes the same step methods as `RouteBuilder` (to, process, filter, etc.)
/// but NOT `.build()` and NOT `.multicast()` (no nested multicasts).
///
/// Calling `.end_multicast()` packages the sub-steps into a `BuilderStep::Multicast`
/// and returns the parent `RouteBuilder`.
pub struct MulticastBuilder {
    parent: RouteBuilder,
    steps: Vec<BuilderStep>,
    config: MulticastConfig,
}

impl MulticastBuilder {
    pub fn parallel(mut self, parallel: bool) -> Self {
        self.config = self.config.parallel(parallel);
        self
    }

    pub fn parallel_limit(mut self, limit: usize) -> Self {
        self.config = self.config.parallel_limit(limit);
        self
    }

    pub fn stop_on_exception(mut self, stop: bool) -> Self {
        self.config = self.config.stop_on_exception(stop);
        self
    }

    pub fn timeout(mut self, duration: std::time::Duration) -> Self {
        self.config = self.config.timeout(duration);
        self
    }

    pub fn aggregation(mut self, strategy: MulticastStrategy) -> Self {
        self.config = self.config.aggregation(strategy);
        self
    }

    pub fn end_multicast(mut self) -> RouteBuilder {
        let step = BuilderStep::Multicast {
            steps: self.steps,
            config: self.config,
        };
        self.parent.steps.push(step);
        self.parent
    }
}

impl StepAccumulator for MulticastBuilder {
    fn steps_mut(&mut self) -> &mut Vec<BuilderStep> {
        &mut self.steps
    }
}

/// Builder for the sub-pipeline within a `.throttle()` ... `.end_throttle()` block.
///
/// Exposes the same step methods as `RouteBuilder` (to, process, filter, etc.)
/// but NOT `.build()` and NOT `.throttle()` (no nested throttles).
///
/// Calling `.end_throttle()` packages the sub-steps into a `BuilderStep::Throttle`
/// and returns the parent `RouteBuilder`.
pub struct ThrottleBuilder {
    parent: RouteBuilder,
    config: ThrottlerConfig,
    steps: Vec<BuilderStep>,
}

impl ThrottleBuilder {
    /// Set the throttle strategy. Default is `Delay`.
    ///
    /// - `Delay`: Queue messages until capacity available
    /// - `Reject`: Return error immediately when throttled
    /// - `Drop`: Silently discard excess messages
    pub fn strategy(mut self, strategy: ThrottleStrategy) -> Self {
        self.config = self.config.strategy(strategy);
        self
    }

    /// Close the throttle scope. Packages the accumulated sub-steps into a
    /// `BuilderStep::Throttle` and returns the parent `RouteBuilder`.
    pub fn end_throttle(mut self) -> RouteBuilder {
        let step = BuilderStep::Throttle {
            config: self.config,
            steps: self.steps,
        };
        self.parent.steps.push(step);
        self.parent
    }
}

impl StepAccumulator for ThrottleBuilder {
    fn steps_mut(&mut self) -> &mut Vec<BuilderStep> {
        &mut self.steps
    }
}

/// Builder for the sub-pipeline within a `.load_balance()` ... `.end_load_balance()` block.
///
/// Exposes the same step methods as `RouteBuilder` (to, process, filter, etc.)
/// but NOT `.build()` and NOT `.load_balance()` (no nested load balancers).
///
/// Calling `.end_load_balance()` packages the sub-steps into a `BuilderStep::LoadBalance`
/// and returns the parent `RouteBuilder`.
pub struct LoadBalancerBuilder {
    parent: RouteBuilder,
    config: LoadBalancerConfig,
    steps: Vec<BuilderStep>,
}

impl LoadBalancerBuilder {
    /// Set the load balance strategy to round-robin (default).
    pub fn round_robin(mut self) -> Self {
        self.config = LoadBalancerConfig::round_robin();
        self
    }

    /// Set the load balance strategy to random selection.
    pub fn random(mut self) -> Self {
        self.config = LoadBalancerConfig::random();
        self
    }

    /// Set the load balance strategy to weighted selection.
    ///
    /// Each endpoint is assigned a weight that determines its probability
    /// of being selected.
    pub fn weighted(mut self, weights: Vec<(String, u32)>) -> Self {
        self.config = LoadBalancerConfig::weighted(weights);
        self
    }

    /// Set the load balance strategy to failover.
    ///
    /// Exchanges are sent to the first endpoint; on failure, the next endpoint
    /// is tried.
    pub fn failover(mut self) -> Self {
        self.config = LoadBalancerConfig::failover();
        self
    }

    /// Enable or disable parallel execution of endpoints.
    ///
    /// When enabled, all endpoints receive the exchange simultaneously.
    /// When disabled (default), only one endpoint is selected per exchange.
    pub fn parallel(mut self, parallel: bool) -> Self {
        self.config = self.config.parallel(parallel);
        self
    }

    /// Close the load balance scope. Packages the accumulated sub-steps into a
    /// `BuilderStep::LoadBalance` and returns the parent `RouteBuilder`.
    pub fn end_load_balance(mut self) -> RouteBuilder {
        let step = BuilderStep::LoadBalance {
            config: self.config,
            steps: self.steps,
        };
        self.parent.steps.push(step);
        self.parent
    }
}

impl StepAccumulator for LoadBalancerBuilder {
    fn steps_mut(&mut self) -> &mut Vec<BuilderStep> {
        &mut self.steps
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::error_handler::ErrorHandlerConfig;
    use camel_api::load_balancer::LoadBalanceStrategy;
    use camel_api::{Exchange, Message};
    use camel_core::route::BuilderStep;
    use std::sync::Arc;
    use std::time::Duration;
    use tower::{Service, ServiceExt};

    #[test]
    fn test_builder_from_creates_definition() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .build()
            .unwrap();
        assert_eq!(definition.from_uri(), "timer:tick");
    }

    #[test]
    fn test_builder_empty_from_uri_errors() {
        let result = RouteBuilder::from("").route_id("test-route").build();
        assert!(result.is_err());
    }

    #[test]
    fn test_builder_to_adds_step() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .to("log:info")
            .build()
            .unwrap();

        assert_eq!(definition.from_uri(), "timer:tick");
        // We can verify steps were added by checking the structure
        assert!(matches!(&definition.steps()[0], BuilderStep::To(uri) if uri == "log:info"));
    }

    #[test]
    fn test_builder_filter_adds_filter_step() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .filter(|_ex| true)
            .to("mock:result")
            .end_filter()
            .build()
            .unwrap();

        assert!(matches!(&definition.steps()[0], BuilderStep::Filter { .. }));
    }

    #[test]
    fn test_builder_set_header_adds_processor_step() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .set_header("key", Value::String("value".into()))
            .build()
            .unwrap();

        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_)));
    }

    #[test]
    fn test_builder_map_body_adds_processor_step() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .map_body(|body| body)
            .build()
            .unwrap();

        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_)));
    }

    #[test]
    fn test_builder_process_adds_processor_step() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .process(|ex| async move { Ok(ex) })
            .build()
            .unwrap();

        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_)));
    }

    #[test]
    fn test_builder_chain_multiple_steps() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .set_header("source", Value::String("timer".into()))
            .filter(|ex| ex.input.header("source").is_some())
            .to("log:info")
            .end_filter()
            .to("mock:result")
            .build()
            .unwrap();

        assert_eq!(definition.steps().len(), 3); // set_header + Filter + To("mock:result")
        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_))); // set_header
        assert!(matches!(&definition.steps()[1], BuilderStep::Filter { .. })); // filter
        assert!(matches!(&definition.steps()[2], BuilderStep::To(uri) if uri == "mock:result"));
    }

    // -----------------------------------------------------------------------
    // Processor behavior tests — exercise the real Tower services directly
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_set_header_processor_works() {
        let mut svc = SetHeader::new(IdentityProcessor, "greeting", Value::String("hello".into()));
        let exchange = Exchange::new(Message::new("test"));
        let result = svc.call(exchange).await.unwrap();
        assert_eq!(
            result.input.header("greeting"),
            Some(&Value::String("hello".into()))
        );
    }

    #[tokio::test]
    async fn test_filter_processor_passes() {
        use camel_api::BoxProcessorExt;
        use camel_processor::FilterService;

        let sub = BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }));
        let mut svc =
            FilterService::new(|ex: &Exchange| ex.input.body.as_text() == Some("pass"), sub);
        let exchange = Exchange::new(Message::new("pass"));
        let result = svc.ready().await.unwrap().call(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("pass"));
    }

    #[tokio::test]
    async fn test_filter_processor_blocks() {
        use camel_api::BoxProcessorExt;
        use camel_processor::FilterService;

        let sub = BoxProcessor::from_fn(|_ex| {
            Box::pin(async move { Err(CamelError::ProcessorError("should not reach".into())) })
        });
        let mut svc =
            FilterService::new(|ex: &Exchange| ex.input.body.as_text() == Some("pass"), sub);
        let exchange = Exchange::new(Message::new("reject"));
        let result = svc.ready().await.unwrap().call(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("reject"));
    }

    #[tokio::test]
    async fn test_map_body_processor_works() {
        let mapper = MapBody::new(IdentityProcessor, |body: Body| {
            if let Some(text) = body.as_text() {
                Body::Text(text.to_uppercase())
            } else {
                body
            }
        });
        let exchange = Exchange::new(Message::new("hello"));
        let result = mapper.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("HELLO"));
    }

    #[tokio::test]
    async fn test_process_custom_processor_works() {
        let processor = ProcessorFn::new(|mut ex: Exchange| async move {
            ex.set_property("custom", Value::Bool(true));
            Ok(ex)
        });
        let exchange = Exchange::new(Message::default());
        let result = processor.oneshot(exchange).await.unwrap();
        assert_eq!(result.property("custom"), Some(&Value::Bool(true)));
    }

    // -----------------------------------------------------------------------
    // Sequential pipeline test
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_compose_pipeline_runs_steps_in_order() {
        use camel_core::route::compose_pipeline;

        let processors = vec![
            BoxProcessor::new(SetHeader::new(
                IdentityProcessor,
                "step",
                Value::String("one".into()),
            )),
            BoxProcessor::new(MapBody::new(IdentityProcessor, |body: Body| {
                if let Some(text) = body.as_text() {
                    Body::Text(format!("{}-processed", text))
                } else {
                    body
                }
            })),
        ];

        let pipeline = compose_pipeline(processors);
        let exchange = Exchange::new(Message::new("hello"));
        let result = pipeline.oneshot(exchange).await.unwrap();

        assert_eq!(
            result.input.header("step"),
            Some(&Value::String("one".into()))
        );
        assert_eq!(result.input.body.as_text(), Some("hello-processed"));
    }

    #[tokio::test]
    async fn test_compose_pipeline_empty_is_identity() {
        use camel_core::route::compose_pipeline;

        let pipeline = compose_pipeline(vec![]);
        let exchange = Exchange::new(Message::new("unchanged"));
        let result = pipeline.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("unchanged"));
    }

    // -----------------------------------------------------------------------
    // Circuit breaker builder tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_builder_circuit_breaker_sets_config() {
        use camel_api::circuit_breaker::CircuitBreakerConfig;

        let config = CircuitBreakerConfig::new().failure_threshold(5);
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .circuit_breaker(config)
            .build()
            .unwrap();

        let cb = definition
            .circuit_breaker_config()
            .expect("circuit breaker should be set");
        assert_eq!(cb.failure_threshold, 5);
    }

    #[test]
    fn test_builder_circuit_breaker_with_error_handler() {
        use camel_api::circuit_breaker::CircuitBreakerConfig;
        use camel_api::error_handler::ErrorHandlerConfig;

        let cb_config = CircuitBreakerConfig::new().failure_threshold(3);
        let eh_config = ErrorHandlerConfig::log_only();

        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .to("log:info")
            .circuit_breaker(cb_config)
            .error_handler(eh_config)
            .build()
            .unwrap();

        assert!(
            definition.circuit_breaker_config().is_some(),
            "circuit breaker config should be set"
        );
        // Route definition was built successfully with both configs.
    }

    #[test]
    fn test_builder_on_exception_shorthand_multiple_clauses_preserve_order() {
        let definition = RouteBuilder::from("direct:start")
            .route_id("test-route")
            .dead_letter_channel("log:dlc")
            .on_exception(|e| matches!(e, CamelError::Io(_)))
            .retry(3)
            .handled_by("log:io")
            .end_on_exception()
            .on_exception(|e| matches!(e, CamelError::ProcessorError(_)))
            .retry(1)
            .end_on_exception()
            .to("mock:out")
            .build()
            .expect("route should build");

        let cfg = definition
            .error_handler_config()
            .expect("error handler should be set");
        assert_eq!(cfg.policies.len(), 2);
        assert_eq!(cfg.dlc_uri.as_deref(), Some("log:dlc"));
        assert_eq!(
            cfg.policies[0].retry.as_ref().map(|p| p.max_attempts),
            Some(3)
        );
        assert_eq!(cfg.policies[0].handled_by.as_deref(), Some("log:io"));
        assert_eq!(
            cfg.policies[1].retry.as_ref().map(|p| p.max_attempts),
            Some(1)
        );
    }

    #[test]
    fn test_builder_on_exception_mixed_mode_rejected() {
        let result = RouteBuilder::from("direct:start")
            .route_id("test-route")
            .error_handler(ErrorHandlerConfig::log_only())
            .on_exception(|_e| true)
            .end_on_exception()
            .to("mock:out")
            .build();

        let err = result.err().expect("mixed mode should fail with an error");

        assert!(
            format!("{err}").contains("mixed error handler modes"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_builder_on_exception_backoff_and_jitter_without_retry_noop() {
        let definition = RouteBuilder::from("direct:start")
            .route_id("test-route")
            .on_exception(|_e| true)
            .with_backoff(Duration::from_millis(5), 3.0, Duration::from_millis(100))
            .with_jitter(0.5)
            .end_on_exception()
            .to("mock:out")
            .build()
            .expect("route should build");

        let cfg = definition
            .error_handler_config()
            .expect("error handler should be set");
        assert_eq!(cfg.policies.len(), 1);
        assert!(cfg.policies[0].retry.is_none());
    }

    #[test]
    fn test_builder_dead_letter_channel_without_on_exception_sets_dlc() {
        let definition = RouteBuilder::from("direct:start")
            .route_id("test-route")
            .dead_letter_channel("log:dlc")
            .to("mock:out")
            .build()
            .expect("route should build");

        let cfg = definition
            .error_handler_config()
            .expect("error handler should be set");
        assert_eq!(cfg.dlc_uri.as_deref(), Some("log:dlc"));
        assert!(cfg.policies.is_empty());
    }

    #[test]
    fn test_builder_dead_letter_channel_called_twice_uses_latest_and_keeps_policies() {
        let definition = RouteBuilder::from("direct:start")
            .route_id("test-route")
            .dead_letter_channel("log:first")
            .on_exception(|e| matches!(e, CamelError::Io(_)))
            .retry(2)
            .end_on_exception()
            .dead_letter_channel("log:second")
            .to("mock:out")
            .build()
            .expect("route should build");

        let cfg = definition
            .error_handler_config()
            .expect("error handler should be set");
        assert_eq!(cfg.dlc_uri.as_deref(), Some("log:second"));
        assert_eq!(cfg.policies.len(), 1);
        assert_eq!(
            cfg.policies[0].retry.as_ref().map(|p| p.max_attempts),
            Some(2)
        );
    }

    #[test]
    fn test_builder_on_exception_without_dlc_defaults_to_log_only() {
        let definition = RouteBuilder::from("direct:start")
            .route_id("test-route")
            .on_exception(|e| matches!(e, CamelError::ProcessorError(_)))
            .retry(1)
            .end_on_exception()
            .to("mock:out")
            .build()
            .expect("route should build");

        let cfg = definition
            .error_handler_config()
            .expect("error handler should be set");
        assert!(cfg.dlc_uri.is_none());
        assert_eq!(cfg.policies.len(), 1);
    }

    #[test]
    fn test_builder_error_handler_explicit_overwrite_stays_explicit_mode() {
        let first = ErrorHandlerConfig::dead_letter_channel("log:first");
        let second = ErrorHandlerConfig::dead_letter_channel("log:second");

        let definition = RouteBuilder::from("direct:start")
            .route_id("test-route")
            .error_handler(first)
            .error_handler(second)
            .to("mock:out")
            .build()
            .expect("route should build");

        let cfg = definition
            .error_handler_config()
            .expect("error handler should be set");
        assert_eq!(cfg.dlc_uri.as_deref(), Some("log:second"));
    }

    // --- Splitter builder tests ---

    #[test]
    fn test_split_builder_typestate() {
        use camel_api::splitter::{SplitterConfig, split_body_lines};

        // .split() returns SplitBuilder, .end_split() returns RouteBuilder
        let definition = RouteBuilder::from("timer:test?period=1000")
            .route_id("test-route")
            .split(SplitterConfig::new(split_body_lines()))
            .to("mock:per-fragment")
            .end_split()
            .to("mock:final")
            .build()
            .unwrap();

        // Should have 2 top-level steps: Split + To("mock:final")
        assert_eq!(definition.steps().len(), 2);
    }

    #[test]
    fn test_split_builder_steps_collected() {
        use camel_api::splitter::{SplitterConfig, split_body_lines};

        let definition = RouteBuilder::from("timer:test?period=1000")
            .route_id("test-route")
            .split(SplitterConfig::new(split_body_lines()))
            .set_header("fragment", Value::String("yes".into()))
            .to("mock:per-fragment")
            .end_split()
            .build()
            .unwrap();

        // Should have 1 top-level step: Split (containing 2 sub-steps)
        assert_eq!(definition.steps().len(), 1);
        match &definition.steps()[0] {
            BuilderStep::Split { steps, .. } => {
                assert_eq!(steps.len(), 2); // SetHeader + To
            }
            other => panic!("Expected Split, got {:?}", other),
        }
    }

    #[test]
    fn test_split_builder_config_propagated() {
        use camel_api::splitter::{AggregationStrategy, SplitterConfig, split_body_lines};

        let definition = RouteBuilder::from("timer:test?period=1000")
            .route_id("test-route")
            .split(
                SplitterConfig::new(split_body_lines())
                    .parallel(true)
                    .parallel_limit(4)
                    .aggregation(AggregationStrategy::CollectAll),
            )
            .to("mock:per-fragment")
            .end_split()
            .build()
            .unwrap();

        match &definition.steps()[0] {
            BuilderStep::Split { config, .. } => {
                assert!(config.parallel);
                assert_eq!(config.parallel_limit, Some(4));
                assert!(matches!(
                    config.aggregation,
                    AggregationStrategy::CollectAll
                ));
            }
            other => panic!("Expected Split, got {:?}", other),
        }
    }

    #[test]
    fn test_aggregate_builder_adds_step() {
        use camel_api::aggregator::AggregatorConfig;
        use camel_core::route::BuilderStep;

        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .aggregate(
                AggregatorConfig::correlate_by("key")
                    .complete_when_size(2)
                    .build(),
            )
            .build()
            .unwrap();

        assert_eq!(definition.steps().len(), 1);
        assert!(matches!(
            definition.steps()[0],
            BuilderStep::Aggregate { .. }
        ));
    }

    #[test]
    fn test_aggregate_in_split_builder() {
        use camel_api::aggregator::AggregatorConfig;
        use camel_api::splitter::{SplitterConfig, split_body_lines};
        use camel_core::route::BuilderStep;

        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .split(SplitterConfig::new(split_body_lines()))
            .aggregate(
                AggregatorConfig::correlate_by("key")
                    .complete_when_size(1)
                    .build(),
            )
            .end_split()
            .build()
            .unwrap();

        assert_eq!(definition.steps().len(), 1);
        if let BuilderStep::Split { steps, .. } = &definition.steps()[0] {
            assert!(matches!(steps[0], BuilderStep::Aggregate { .. }));
        } else {
            panic!("expected Split step");
        }
    }

    // ── set_body / set_body_fn / set_header_fn builder tests ────────────────────

    #[test]
    fn test_builder_set_body_static_adds_processor() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .set_body("fixed")
            .build()
            .unwrap();
        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_)));
    }

    #[test]
    fn test_builder_set_body_fn_adds_processor() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .set_body_fn(|_ex: &Exchange| Body::Text("dynamic".into()))
            .build()
            .unwrap();
        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_)));
    }

    #[test]
    fn transform_alias_produces_same_as_set_body() {
        let route_transform = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .transform("hello")
            .build()
            .unwrap();

        let route_set_body = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .set_body("hello")
            .build()
            .unwrap();

        assert_eq!(route_transform.steps().len(), route_set_body.steps().len());
    }

    #[test]
    fn test_builder_set_header_fn_adds_processor() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .set_header_fn("k", |_ex: &Exchange| Value::String("v".into()))
            .build()
            .unwrap();
        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_)));
    }

    #[tokio::test]
    async fn test_set_body_static_processor_works() {
        use camel_core::route::compose_pipeline;
        let def = RouteBuilder::from("t:t")
            .route_id("test-route")
            .set_body("replaced")
            .build()
            .unwrap();
        let pipeline = compose_pipeline(
            def.steps()
                .iter()
                .filter_map(|s| {
                    if let BuilderStep::Processor(p) = s {
                        Some(p.clone())
                    } else {
                        None
                    }
                })
                .collect(),
        );
        let exchange = Exchange::new(Message::new("original"));
        let result = pipeline.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("replaced"));
    }

    #[tokio::test]
    async fn test_set_body_fn_processor_works() {
        use camel_core::route::compose_pipeline;
        let def = RouteBuilder::from("t:t")
            .route_id("test-route")
            .set_body_fn(|ex: &Exchange| {
                Body::Text(ex.input.body.as_text().unwrap_or("").to_uppercase())
            })
            .build()
            .unwrap();
        let pipeline = compose_pipeline(
            def.steps()
                .iter()
                .filter_map(|s| {
                    if let BuilderStep::Processor(p) = s {
                        Some(p.clone())
                    } else {
                        None
                    }
                })
                .collect(),
        );
        let exchange = Exchange::new(Message::new("hello"));
        let result = pipeline.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("HELLO"));
    }

    #[tokio::test]
    async fn test_set_header_fn_processor_works() {
        use camel_core::route::compose_pipeline;
        let def = RouteBuilder::from("t:t")
            .route_id("test-route")
            .set_header_fn("echo", |ex: &Exchange| {
                ex.input
                    .body
                    .as_text()
                    .map(|t| Value::String(t.into()))
                    .unwrap_or(Value::Null)
            })
            .build()
            .unwrap();
        let pipeline = compose_pipeline(
            def.steps()
                .iter()
                .filter_map(|s| {
                    if let BuilderStep::Processor(p) = s {
                        Some(p.clone())
                    } else {
                        None
                    }
                })
                .collect(),
        );
        let exchange = Exchange::new(Message::new("ping"));
        let result = pipeline.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.header("echo"),
            Some(&Value::String("ping".into()))
        );
    }

    // ── FilterBuilder typestate tests ─────────────────────────────────────

    #[test]
    fn test_filter_builder_typestate() {
        let result = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
            .route_id("test-route")
            .filter(|_ex| true)
            .to("mock:inner")
            .end_filter()
            .to("mock:outer")
            .build();
        assert!(result.is_ok());
    }

    #[test]
    fn test_filter_builder_steps_collected() {
        let definition = RouteBuilder::from("timer:tick?period=50&repeatCount=1")
            .route_id("test-route")
            .filter(|_ex| true)
            .to("mock:inner")
            .end_filter()
            .build()
            .unwrap();

        assert_eq!(definition.steps().len(), 1);
        assert!(matches!(&definition.steps()[0], BuilderStep::Filter { .. }));
    }

    #[test]
    fn test_wire_tap_builder_adds_step() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .wire_tap("mock:tap")
            .to("mock:result")
            .build()
            .unwrap();

        assert_eq!(definition.steps().len(), 2);
        assert!(
            matches!(&definition.steps()[0], BuilderStep::WireTap { uri } if uri == "mock:tap")
        );
        assert!(matches!(&definition.steps()[1], BuilderStep::To(uri) if uri == "mock:result"));
    }

    // ── MulticastBuilder typestate tests ─────────────────────────────────────

    #[test]
    fn test_multicast_builder_typestate() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .multicast()
            .to("direct:a")
            .to("direct:b")
            .end_multicast()
            .to("mock:result")
            .build()
            .unwrap();

        assert_eq!(definition.steps().len(), 2); // Multicast + To("mock:result")
    }

    #[test]
    fn test_multicast_builder_steps_collected() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .multicast()
            .to("direct:a")
            .to("direct:b")
            .end_multicast()
            .build()
            .unwrap();

        match &definition.steps()[0] {
            BuilderStep::Multicast { steps, .. } => {
                assert_eq!(steps.len(), 2);
            }
            other => panic!("Expected Multicast, got {:?}", other),
        }
    }

    // ── Concurrency builder tests ─────────────────────────────────────

    #[test]
    fn test_builder_concurrent_sets_concurrency() {
        use camel_component_api::ConcurrencyModel;

        let definition = RouteBuilder::from("http://0.0.0.0:8080/test")
            .route_id("test-route")
            .concurrent(16)
            .to("log:info")
            .build()
            .unwrap();

        assert_eq!(
            definition.concurrency_override(),
            Some(&ConcurrencyModel::Concurrent { max: Some(16) })
        );
    }

    #[test]
    fn test_builder_concurrent_zero_means_unbounded() {
        use camel_component_api::ConcurrencyModel;

        let definition = RouteBuilder::from("http://0.0.0.0:8080/test")
            .route_id("test-route")
            .concurrent(0)
            .to("log:info")
            .build()
            .unwrap();

        assert_eq!(
            definition.concurrency_override(),
            Some(&ConcurrencyModel::Concurrent { max: None })
        );
    }

    #[test]
    fn test_builder_sequential_sets_concurrency() {
        use camel_component_api::ConcurrencyModel;

        let definition = RouteBuilder::from("http://0.0.0.0:8080/test")
            .route_id("test-route")
            .sequential()
            .to("log:info")
            .build()
            .unwrap();

        assert_eq!(
            definition.concurrency_override(),
            Some(&ConcurrencyModel::Sequential)
        );
    }

    #[test]
    fn test_builder_default_concurrency_is_none() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .to("log:info")
            .build()
            .unwrap();

        assert_eq!(definition.concurrency_override(), None);
    }

    // ── Route lifecycle builder tests ─────────────────────────────────────

    #[test]
    fn test_builder_route_id_sets_id() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("my-route")
            .build()
            .unwrap();

        assert_eq!(definition.route_id(), "my-route");
    }

    #[test]
    fn test_build_without_route_id_fails() {
        let result = RouteBuilder::from("timer:tick?period=1000")
            .to("log:info")
            .build();
        let err = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("build() should fail without route_id"),
        };
        assert!(
            err.contains("route_id"),
            "error should mention route_id, got: {}",
            err
        );
    }

    #[test]
    fn test_builder_auto_startup_false() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .auto_startup(false)
            .build()
            .unwrap();

        assert!(!definition.auto_startup());
    }

    #[test]
    fn test_builder_startup_order_custom() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .startup_order(50)
            .build()
            .unwrap();

        assert_eq!(definition.startup_order(), 50);
    }

    #[test]
    fn test_builder_defaults() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .build()
            .unwrap();

        assert_eq!(definition.route_id(), "test-route");
        assert!(definition.auto_startup());
        assert_eq!(definition.startup_order(), 1000);
    }

    // ── Choice typestate tests ──────────────────────────────────────────────────

    #[test]
    fn test_choice_builder_single_when() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .choice()
            .when(|ex: &Exchange| ex.input.header("type").is_some())
            .to("mock:typed")
            .end_when()
            .end_choice()
            .build()
            .unwrap();
        assert_eq!(definition.steps().len(), 1);
        assert!(
            matches!(&definition.steps()[0], BuilderStep::Choice { whens, otherwise }
            if whens.len() == 1 && otherwise.is_none())
        );
    }

    #[test]
    fn test_choice_builder_when_otherwise() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .choice()
            .when(|ex: &Exchange| ex.input.header("a").is_some())
            .to("mock:a")
            .end_when()
            .otherwise()
            .to("mock:fallback")
            .end_otherwise()
            .end_choice()
            .build()
            .unwrap();
        assert!(
            matches!(&definition.steps()[0], BuilderStep::Choice { whens, otherwise }
            if whens.len() == 1 && otherwise.is_some())
        );
    }

    #[test]
    fn test_choice_builder_multiple_whens() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .choice()
            .when(|ex: &Exchange| ex.input.header("a").is_some())
            .to("mock:a")
            .end_when()
            .when(|ex: &Exchange| ex.input.header("b").is_some())
            .to("mock:b")
            .end_when()
            .end_choice()
            .build()
            .unwrap();
        assert!(
            matches!(&definition.steps()[0], BuilderStep::Choice { whens, .. }
            if whens.len() == 2)
        );
    }

    #[test]
    fn test_choice_step_after_choice() {
        // Steps after end_choice() are added to the outer pipeline, not inside choice.
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .choice()
            .when(|_ex: &Exchange| true)
            .to("mock:inner")
            .end_when()
            .end_choice()
            .to("mock:outer") // must be step[1], not inside choice
            .build()
            .unwrap();
        assert_eq!(definition.steps().len(), 2);
        assert!(matches!(&definition.steps()[1], BuilderStep::To(uri) if uri == "mock:outer"));
    }

    // ── Throttle typestate tests ──────────────────────────────────────────────────

    #[test]
    fn test_throttle_builder_typestate() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .throttle(10, std::time::Duration::from_secs(1))
            .to("mock:result")
            .end_throttle()
            .build()
            .unwrap();

        assert_eq!(definition.steps().len(), 1);
        assert!(matches!(
            &definition.steps()[0],
            BuilderStep::Throttle { .. }
        ));
    }

    #[test]
    fn test_throttle_builder_with_strategy() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .throttle(10, std::time::Duration::from_secs(1))
            .strategy(ThrottleStrategy::Reject)
            .to("mock:result")
            .end_throttle()
            .build()
            .unwrap();

        if let BuilderStep::Throttle { config, .. } = &definition.steps()[0] {
            assert_eq!(config.strategy, ThrottleStrategy::Reject);
        } else {
            panic!("Expected Throttle step");
        }
    }

    #[test]
    fn test_throttle_builder_steps_collected() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .throttle(5, std::time::Duration::from_secs(1))
            .set_header("throttled", Value::Bool(true))
            .to("mock:throttled")
            .end_throttle()
            .build()
            .unwrap();

        match &definition.steps()[0] {
            BuilderStep::Throttle { steps, .. } => {
                assert_eq!(steps.len(), 2); // SetHeader + To
            }
            other => panic!("Expected Throttle, got {:?}", other),
        }
    }

    #[test]
    fn test_throttle_step_after_throttle() {
        // Steps after end_throttle() are added to the outer pipeline, not inside throttle.
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .throttle(10, std::time::Duration::from_secs(1))
            .to("mock:inner")
            .end_throttle()
            .to("mock:outer")
            .build()
            .unwrap();

        assert_eq!(definition.steps().len(), 2);
        assert!(matches!(&definition.steps()[1], BuilderStep::To(uri) if uri == "mock:outer"));
    }

    // ── LoadBalance typestate tests ──────────────────────────────────────────────────

    #[test]
    fn test_load_balance_builder_typestate() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .load_balance()
            .round_robin()
            .to("mock:a")
            .to("mock:b")
            .end_load_balance()
            .build()
            .unwrap();

        assert_eq!(definition.steps().len(), 1);
        assert!(matches!(
            &definition.steps()[0],
            BuilderStep::LoadBalance { .. }
        ));
    }

    #[test]
    fn test_load_balance_builder_with_strategy() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .load_balance()
            .random()
            .to("mock:result")
            .end_load_balance()
            .build()
            .unwrap();

        if let BuilderStep::LoadBalance { config, .. } = &definition.steps()[0] {
            assert_eq!(config.strategy, LoadBalanceStrategy::Random);
        } else {
            panic!("Expected LoadBalance step");
        }
    }

    #[test]
    fn test_load_balance_builder_steps_collected() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .load_balance()
            .set_header("lb", Value::Bool(true))
            .to("mock:a")
            .end_load_balance()
            .build()
            .unwrap();

        match &definition.steps()[0] {
            BuilderStep::LoadBalance { steps, .. } => {
                assert_eq!(steps.len(), 2); // SetHeader + To
            }
            other => panic!("Expected LoadBalance, got {:?}", other),
        }
    }

    #[test]
    fn test_load_balance_step_after_load_balance() {
        // Steps after end_load_balance() are added to the outer pipeline, not inside load_balance.
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .load_balance()
            .to("mock:inner")
            .end_load_balance()
            .to("mock:outer")
            .build()
            .unwrap();

        assert_eq!(definition.steps().len(), 2);
        assert!(matches!(&definition.steps()[1], BuilderStep::To(uri) if uri == "mock:outer"));
    }

    // ── DynamicRouter typestate tests ──────────────────────────────────────────────────

    #[test]
    fn test_dynamic_router_builder() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .dynamic_router(Arc::new(|_| Some("mock:result".to_string())))
            .build()
            .unwrap();

        assert_eq!(definition.steps().len(), 1);
        assert!(matches!(
            &definition.steps()[0],
            BuilderStep::DynamicRouter { .. }
        ));
    }

    #[test]
    fn test_dynamic_router_builder_with_config() {
        let config = DynamicRouterConfig::new(Arc::new(|_| Some("mock:a".to_string())))
            .max_iterations(100)
            .cache_size(500);

        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .dynamic_router_with_config(config)
            .build()
            .unwrap();

        assert_eq!(definition.steps().len(), 1);
        if let BuilderStep::DynamicRouter { config } = &definition.steps()[0] {
            assert_eq!(config.max_iterations, 100);
            assert_eq!(config.cache_size, 500);
        } else {
            panic!("Expected DynamicRouter step");
        }
    }

    #[test]
    fn test_dynamic_router_step_after_router() {
        // Steps after dynamic_router() are added to the outer pipeline.
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .dynamic_router(Arc::new(|_| Some("mock:inner".to_string())))
            .to("mock:outer")
            .build()
            .unwrap();

        assert_eq!(definition.steps().len(), 2);
        assert!(matches!(
            &definition.steps()[0],
            BuilderStep::DynamicRouter { .. }
        ));
        assert!(matches!(&definition.steps()[1], BuilderStep::To(uri) if uri == "mock:outer"));
    }

    #[test]
    fn routing_slip_builder_creates_step() {
        use camel_api::RoutingSlipExpression;

        let expression: RoutingSlipExpression = Arc::new(|_| Some("direct:a,direct:b".to_string()));

        let route = RouteBuilder::from("direct:start")
            .route_id("routing-slip-test")
            .routing_slip(expression)
            .build()
            .unwrap();

        assert!(
            matches!(route.steps()[0], BuilderStep::RoutingSlip { .. }),
            "Expected RoutingSlip step"
        );
    }

    #[test]
    fn routing_slip_with_config_builder_creates_step() {
        use camel_api::RoutingSlipConfig;

        let config = RoutingSlipConfig::new(Arc::new(|_| Some("mock:a".to_string())))
            .uri_delimiter("|")
            .cache_size(50)
            .ignore_invalid_endpoints(true);

        let route = RouteBuilder::from("direct:start")
            .route_id("routing-slip-config-test")
            .routing_slip_with_config(config)
            .build()
            .unwrap();

        if let BuilderStep::RoutingSlip { config } = &route.steps()[0] {
            assert_eq!(config.uri_delimiter, "|");
            assert_eq!(config.cache_size, 50);
            assert!(config.ignore_invalid_endpoints);
        } else {
            panic!("Expected RoutingSlip step");
        }
    }

    #[test]
    fn test_builder_marshal_adds_processor_step() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .marshal("json")
            .build()
            .unwrap();
        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_)));
    }

    #[test]
    fn test_builder_unmarshal_adds_processor_step() {
        let definition = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .unmarshal("json")
            .build()
            .unwrap();
        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_)));
    }

    #[test]
    #[should_panic(expected = "unknown data format: 'csv'")]
    fn test_builder_marshal_panics_on_unknown_format() {
        let _ = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .marshal("csv")
            .build();
    }

    #[test]
    #[should_panic(expected = "unknown data format: 'csv'")]
    fn test_builder_unmarshal_panics_on_unknown_format() {
        let _ = RouteBuilder::from("timer:tick")
            .route_id("test-route")
            .unmarshal("csv")
            .build();
    }
}
