use camel_api::body::Body;
use camel_api::{BoxProcessor, CamelError, Exchange, IdentityProcessor, ProcessorFn, Value};
use camel_core::route::{BuilderStep, RouteDefinition};
use camel_processor::{Filter, MapBody, SetHeader};

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
}

impl RouteBuilder {
    /// Start building a route from the given source endpoint URI.
    pub fn from(endpoint: &str) -> Self {
        Self {
            from_uri: endpoint.to_string(),
            steps: Vec::new(),
        }
    }

    /// Add a filter step. Exchanges that do **not** match the predicate are
    /// passed through unchanged (not forwarded to the inner processor).
    pub fn filter<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&Exchange) -> bool + Clone + Send + Sync + 'static,
    {
        let svc = Filter::new(IdentityProcessor, predicate);
        self.steps
            .push(BuilderStep::Processor(BoxProcessor::new(svc)));
        self
    }

    /// Add a processor step defined by an arbitrary async closure.
    pub fn process<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(Exchange) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Exchange, CamelError>> + Send + 'static,
    {
        let svc = ProcessorFn::new(f);
        self.steps
            .push(BuilderStep::Processor(BoxProcessor::new(svc)));
        self
    }

    /// Add a step that sets a header on the exchange's input message.
    pub fn set_header(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        let svc = SetHeader::new(IdentityProcessor, key, value);
        self.steps
            .push(BuilderStep::Processor(BoxProcessor::new(svc)));
        self
    }

    /// Add a step that transforms the message body.
    pub fn map_body<F>(mut self, mapper: F) -> Self
    where
        F: Fn(Body) -> Body + Clone + Send + Sync + 'static,
    {
        let svc = MapBody::new(IdentityProcessor, mapper);
        self.steps
            .push(BuilderStep::Processor(BoxProcessor::new(svc)));
        self
    }

    /// Add a "to" destination step that sends the exchange to the given
    /// endpoint URI. The actual send is handled at runtime when the
    /// CamelContext resolves the RouteDefinition.
    pub fn to(mut self, endpoint: &str) -> Self {
        self.steps.push(BuilderStep::To(endpoint.to_string()));
        self
    }

    /// Consume the builder and produce a [`RouteDefinition`].
    pub fn build(self) -> Result<RouteDefinition, CamelError> {
        if self.from_uri.is_empty() {
            return Err(CamelError::RouteError(
                "route must have a 'from' URI".to_string(),
            ));
        }
        Ok(RouteDefinition::new(self.from_uri, self.steps))
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
    fn test_builder_filter_adds_processor_step() {
        let definition = RouteBuilder::from("timer:tick")
            .filter(|_ex| true)
            .build()
            .unwrap();

        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_)));
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
            .to("mock:result")
            .build()
            .unwrap();

        assert_eq!(definition.steps().len(), 4);
        assert!(matches!(&definition.steps()[0], BuilderStep::Processor(_))); // set_header
        assert!(matches!(&definition.steps()[1], BuilderStep::Processor(_))); // filter
        assert!(matches!(&definition.steps()[2], BuilderStep::To(uri) if uri == "log:info"));
        assert!(matches!(&definition.steps()[3], BuilderStep::To(uri) if uri == "mock:result"));
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
        let filter = Filter::new(IdentityProcessor, |ex: &Exchange| {
            ex.input.body.as_text() == Some("pass")
        });
        let exchange = Exchange::new(Message::new("pass"));
        let result = filter.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("pass"));
    }

    #[tokio::test]
    async fn test_filter_processor_blocks() {
        let filter = Filter::new(IdentityProcessor, |ex: &Exchange| {
            ex.input.body.as_text() == Some("pass")
        });
        let exchange = Exchange::new(Message::new("reject"));
        let result = filter.oneshot(exchange).await.unwrap();
        // Exchange is returned as-is (filter does not forward to inner)
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
}
