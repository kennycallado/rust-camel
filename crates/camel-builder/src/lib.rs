use camel_api::aggregator::AggregatorConfig;
use camel_api::body::Body;
use camel_api::circuit_breaker::CircuitBreakerConfig;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::multicast::{MulticastConfig, MulticastStrategy};
use camel_api::splitter::SplitterConfig;
use camel_api::{
    BoxProcessor, CamelError, Exchange, FilterPredicate, IdentityProcessor, ProcessorFn, Value,
};
use camel_core::route::{BuilderStep, RouteDefinition};
use camel_processor::{DynamicSetHeader, MapBody, SetBody, SetHeader, StopService};

/// Shared step-accumulation methods for all builder types.
///
/// Implementors provide `steps_mut()` and get 9 step-adding methods for free.
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
}

impl RouteBuilder {
    /// Start building a route from the given source endpoint URI.
    pub fn from(endpoint: &str) -> Self {
        Self {
            from_uri: endpoint.to_string(),
            steps: Vec::new(),
            error_handler: None,
            circuit_breaker_config: None,
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

    /// Consume the builder and produce a [`RouteDefinition`].
    pub fn build(self) -> Result<RouteDefinition, CamelError> {
        if self.from_uri.is_empty() {
            return Err(CamelError::RouteError(
                "route must have a 'from' URI".to_string(),
            ));
        }
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Exchange, Message};
    use camel_core::route::BuilderStep;
    use tower::{Service, ServiceExt};

    #[test]
    fn test_builder_from_creates_definition() {
        let definition = RouteBuilder::from("timer:tick").build().unwrap();
        assert_eq!(definition.from_uri(), "timer:tick");
    }

    #[test]
    fn test_builder_empty_from_uri_errors() {
        let result = RouteBuilder::from("").build();
        assert!(result.is_err());
    }

    #[test]
    fn test_builder_to_adds_step() {
        let definition = RouteBuilder::from("timer:tick")
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
            .set_header("key", Value::String("value".into()))
            .build()
            .unwrap();

        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_)));
    }

    #[test]
    fn test_builder_map_body_adds_processor_step() {
        let definition = RouteBuilder::from("timer:tick")
            .map_body(|body| body)
            .build()
            .unwrap();

        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_)));
    }

    #[test]
    fn test_builder_process_adds_processor_step() {
        let definition = RouteBuilder::from("timer:tick")
            .process(|ex| async move { Ok(ex) })
            .build()
            .unwrap();

        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_)));
    }

    #[test]
    fn test_builder_chain_multiple_steps() {
        let definition = RouteBuilder::from("timer:tick")
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
            .set_body("fixed")
            .build()
            .unwrap();
        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_)));
    }

    #[test]
    fn test_builder_set_body_fn_adds_processor() {
        let definition = RouteBuilder::from("timer:tick")
            .set_body_fn(|_ex: &Exchange| Body::Text("dynamic".into()))
            .build()
            .unwrap();
        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_)));
    }

    #[test]
    fn test_builder_set_header_fn_adds_processor() {
        let definition = RouteBuilder::from("timer:tick")
            .set_header_fn("k", |_ex: &Exchange| Value::String("v".into()))
            .build()
            .unwrap();
        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_)));
    }

    #[tokio::test]
    async fn test_set_body_static_processor_works() {
        use camel_core::route::compose_pipeline;
        let def = RouteBuilder::from("t:t")
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
}
