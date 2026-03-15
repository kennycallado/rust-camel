use camel_api::aggregator::AggregatorConfig;
use camel_api::body::Body;
use camel_api::body_converter::BodyType;
use camel_api::circuit_breaker::CircuitBreakerConfig;
use camel_api::dynamic_router::{DynamicRouterConfig, RouterExpression};
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::load_balancer::LoadBalancerConfig;
use camel_api::multicast::{MulticastConfig, MulticastStrategy};
use camel_api::splitter::SplitterConfig;
use camel_api::throttler::{ThrottleStrategy, ThrottlerConfig};
use camel_api::{
    BoxProcessor, CamelError, Exchange, FilterPredicate, IdentityProcessor, ProcessorFn, Value,
};
use camel_component::ConcurrencyModel;
use camel_core::route::{BuilderStep, RouteDefinition, WhenStep};
use camel_processor::{
    ConvertBodyTo, DynamicSetHeader, LogLevel, MapBody, SetBody, SetHeader, StopService,
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
        self.steps_mut()
            .push(BuilderStep::Processor(BoxProcessor::new(StopService)));
        self
    }

    /// Log a message at the specified level.
    ///
    /// The message will be logged when an exchange passes through this step.
    fn log(mut self, message: impl Into<String>, level: LogLevel) -> Self {
        use camel_processor::LogProcessor;
        let svc = LogProcessor::new(level, message.into());
        self.steps_mut()
            .push(BuilderStep::Processor(BoxProcessor::new(svc)));
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
    circuit_breaker_config: Option<CircuitBreakerConfig>,
    concurrency: Option<ConcurrencyModel>,
    route_id: Option<String>,
    auto_startup: Option<bool>,
    startup_order: Option<i32>,
}

impl RouteBuilder {
    /// Start building a route from the given source endpoint URI.
    pub fn from(endpoint: &str) -> Self {
        Self {
            from_uri: endpoint.to_string(),
            steps: Vec::new(),
            error_handler: None,
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
        self.error_handler = Some(config);
        self
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
        let definition = RouteDefinition::new(self.from_uri, self.steps);
        let definition = if let Some(eh) = self.error_handler {
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
    use camel_api::load_balancer::LoadBalanceStrategy;
    use camel_api::{Exchange, Message};
    use camel_core::route::BuilderStep;
    use std::sync::Arc;
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
        use camel_component::ConcurrencyModel;

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
        use camel_component::ConcurrencyModel;

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
        use camel_component::ConcurrencyModel;

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
}
