# Tower Reconnection Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reconnect the rust-camel runtime with Tower, resolving all 7 architectural deviations identified in the gap analysis.

**Architecture:** Replace the flat `ProcessorFn`/`RouteStep` dispatcher with real Tower `Service<Exchange>` composition. The builder creates Tower Services (Filter, SetHeader, MapBody), producers implement `Service<Exchange>` directly (no adapter), everything wraps into `BoxProcessor`, and the context calls a single `Service::call()` per exchange. MockComponent gets a shared registry. Integration tests move to `crates/camel-test/`.

**Tech Stack:** Tower 0.5 (Service, Layer, ServiceBuilder, BoxCloneService), tokio 1, async-trait 0.1

---

### Task 1: Add `BoxProcessor` Type Alias and `ProcessorFn` Bridge to `camel-api`

**Files:**
- Modify: `crates/camel-api/src/processor.rs:1-41`
- Modify: `crates/camel-api/src/lib.rs:13`
- Modify: `crates/camel-api/Cargo.toml` (add `tower` `util` feature)

**Step 1: Update Cargo.toml to enable tower `util` feature**

In `crates/camel-api/Cargo.toml`, ensure tower dependency has `util` feature:

```toml
tower = { workspace = true, features = ["util"] }
```

**Step 2: Add BoxProcessor type alias and ProcessorFn adapter**

Add to `crates/camel-api/src/processor.rs` after `IdentityProcessor`:

```rust
/// A type-erased, cloneable processor. This is the main runtime representation
/// of a processor pipeline — a composed chain of Tower Services erased to a
/// single boxed type.
pub type BoxProcessor = tower::util::BoxCloneService<Exchange, Exchange, CamelError>;

/// Adapts an `Fn(Exchange) -> Future<Result<Exchange>>` closure into a Tower Service.
/// This allows user-provided async closures (via `.process()`) to participate
/// in the Tower pipeline.
#[derive(Clone)]
pub struct ProcessorFn<F> {
    f: Arc<F>,
}

impl<F> ProcessorFn<F> {
    pub fn new(f: F) -> Self {
        Self { f: Arc::new(f) }
    }
}

impl<F, Fut> Service<Exchange> for ProcessorFn<F>
where
    F: Fn(Exchange) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Exchange, CamelError>> + Send + 'static,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let f = Arc::clone(&self.f);
        Box::pin(async move { f(exchange).await })
    }
}
```

Add `use std::sync::Arc;` to imports.

**Step 3: Update re-exports in lib.rs**

In `crates/camel-api/src/lib.rs`, change line 13:

```rust
pub use processor::{BoxProcessor, IdentityProcessor, Processor, ProcessorFn};
```

**Step 4: Run tests**

Run: `cargo test -p camel-api`
Expected: All existing tests pass. `BoxProcessor` and `ProcessorFn` compile.

**Step 5: Commit**

```
feat(api): add BoxProcessor type alias and ProcessorFn adapter
```

---

### Task 2: Add Layer Types to `camel-processor`

**Files:**
- Modify: `crates/camel-processor/src/filter.rs` (add `FilterLayer`)
- Modify: `crates/camel-processor/src/set_header.rs` (add `SetHeaderLayer`)
- Modify: `crates/camel-processor/src/map_body.rs` (add `MapBodyLayer`)
- Modify: `crates/camel-processor/src/lib.rs` (update re-exports)
- Modify: `crates/camel-processor/Cargo.toml` (ensure `tower` has `util` feature)

**Step 1: Write test for FilterLayer**

Add to end of `crates/camel-processor/src/filter.rs` tests module:

```rust
#[tokio::test]
async fn test_filter_layer_composes() {
    use tower::ServiceBuilder;

    let svc = ServiceBuilder::new()
        .layer(super::FilterLayer::new(|ex: &Exchange| {
            ex.input.header("active").is_some()
        }))
        .service(IdentityProcessor);

    let mut exchange = Exchange::new(Message::default());
    exchange.input.set_header("active", Value::Bool(true));

    let result = svc.oneshot(exchange).await.unwrap();
    assert!(result.input.header("active").is_some());
}

#[tokio::test]
async fn test_filter_layer_blocks() {
    use tower::ServiceBuilder;

    let svc = ServiceBuilder::new()
        .layer(super::FilterLayer::new(|ex: &Exchange| {
            ex.input.header("active").is_some()
        }))
        .service(IdentityProcessor);

    let exchange = Exchange::new(Message::new("no header"));
    let result = svc.oneshot(exchange).await.unwrap();
    assert_eq!(result.input.body.as_text(), Some("no header"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p camel-processor -- test_filter_layer`
Expected: FAIL — `FilterLayer` does not exist yet.

**Step 3: Implement FilterLayer**

Add to `crates/camel-processor/src/filter.rs` after the `Filter` impl block:

```rust
/// A Tower Layer that wraps an inner service with a [`Filter`].
#[derive(Clone)]
pub struct FilterLayer<F> {
    predicate: F,
}

impl<F> FilterLayer<F> {
    /// Create a new FilterLayer with the given predicate.
    pub fn new(predicate: F) -> Self {
        Self { predicate }
    }
}

impl<S, F> tower::Layer<S> for FilterLayer<F>
where
    F: Clone,
{
    type Service = Filter<S, F>;

    fn layer(&self, inner: S) -> Self::Service {
        Filter::new(inner, self.predicate.clone())
    }
}
```

**Step 4: Implement SetHeaderLayer**

Add to `crates/camel-processor/src/set_header.rs`:

```rust
/// A Tower Layer that wraps an inner service with a [`SetHeader`].
#[derive(Clone)]
pub struct SetHeaderLayer {
    key: String,
    value: Value,
}

impl SetHeaderLayer {
    pub fn new(key: impl Into<String>, value: impl Into<Value>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

impl<S> tower::Layer<S> for SetHeaderLayer {
    type Service = SetHeader<S>;

    fn layer(&self, inner: S) -> Self::Service {
        SetHeader::new(inner, self.key.clone(), self.value.clone())
    }
}
```

Add test:

```rust
#[tokio::test]
async fn test_set_header_layer_composes() {
    use tower::ServiceBuilder;

    let svc = ServiceBuilder::new()
        .layer(super::SetHeaderLayer::new("env", Value::String("test".into())))
        .service(IdentityProcessor);

    let exchange = Exchange::new(Message::default());
    let result = svc.oneshot(exchange).await.unwrap();
    assert_eq!(
        result.input.header("env"),
        Some(&Value::String("test".into()))
    );
}
```

**Step 5: Implement MapBodyLayer**

Add to `crates/camel-processor/src/map_body.rs`:

```rust
/// A Tower Layer that wraps an inner service with a [`MapBody`].
#[derive(Clone)]
pub struct MapBodyLayer<F> {
    mapper: F,
}

impl<F> MapBodyLayer<F> {
    pub fn new(mapper: F) -> Self {
        Self { mapper }
    }
}

impl<S, F> tower::Layer<S> for MapBodyLayer<F>
where
    F: Clone,
{
    type Service = MapBody<S, F>;

    fn layer(&self, inner: S) -> Self::Service {
        MapBody::new(inner, self.mapper.clone())
    }
}
```

Add test:

```rust
#[tokio::test]
async fn test_map_body_layer_composes() {
    use tower::ServiceBuilder;

    let svc = ServiceBuilder::new()
        .layer(super::MapBodyLayer::new(|body: Body| {
            if let Some(text) = body.as_text() {
                Body::Text(text.to_uppercase())
            } else {
                body
            }
        }))
        .service(IdentityProcessor);

    let exchange = Exchange::new(Message::new("hello"));
    let result = svc.oneshot(exchange).await.unwrap();
    assert_eq!(result.input.body.as_text(), Some("HELLO"));
}
```

**Step 6: Update re-exports in lib.rs**

```rust
pub mod filter;
pub mod map_body;
pub mod set_header;

pub use filter::{Filter, FilterLayer};
pub use map_body::{MapBody, MapBodyLayer};
pub use set_header::{SetHeader, SetHeaderLayer};
```

**Step 7: Run all tests**

Run: `cargo test -p camel-processor`
Expected: All tests pass (9 existing + 4 new Layer tests).

**Step 8: Commit**

```
feat(processor): add Tower Layer types for Filter, SetHeader, MapBody
```

---

### Task 3: Make Producers Tower-Native (Remove `Producer` Trait)

**Files:**
- Modify: `crates/camel-component/src/producer.rs` (remove `Producer` trait)
- Modify: `crates/camel-component/src/endpoint.rs` (change `create_producer` return type)
- Modify: `crates/camel-component/src/lib.rs` (remove `Producer` re-export)
- Modify: `crates/camel-component/Cargo.toml` (add tower, camel-api deps if needed)
- Modify: `crates/components/camel-mock/src/lib.rs` (implement `Service<Exchange>` on MockProducer)
- Modify: `crates/components/camel-log/src/lib.rs` (implement `Service<Exchange>` on LogProducer)
- Modify: `crates/components/camel-direct/src/lib.rs` (implement `Service<Exchange>` on DirectProducer)
- Modify: `crates/camel-core/Cargo.toml` (add tower dependency)

**Step 1: Update `Endpoint` trait**

Change `crates/camel-component/src/endpoint.rs` — `create_producer` now returns `BoxProcessor`:

```rust
use camel_api::{BoxProcessor, CamelError};

pub trait Endpoint: Send + Sync {
    fn uri(&self) -> &str;
    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError>;
    fn create_producer(&self) -> Result<BoxProcessor, CamelError>;
}
```

**Step 2: Remove `Producer` trait**

Delete or empty `crates/camel-component/src/producer.rs`. Remove `pub use producer::Producer` from `lib.rs`.

**Step 3: Update each component's producer to implement `Service<Exchange>`**

Each component (mock, log, direct) changes its producer from:
```rust
#[async_trait]
impl Producer for XxxProducer { async fn process(&self, exchange) -> ... }
```
To:
```rust
impl Service<Exchange> for XxxProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;
    fn poll_ready(...) -> Poll<...> { Poll::Ready(Ok(())) }
    fn call(&mut self, exchange: Exchange) -> Self::Future { ... }
}
```
And `create_producer` returns `BoxProcessor::new(XxxProducer { ... })`.

**Step 4: Run check**

Run: `cargo check --workspace`
Expected: May have compilation errors in `camel-core/context.rs` (still references old `Producer`). Fix forward.

**Step 5: Commit**

```
refactor(component): make producers Tower-native, remove Producer async_trait
```

---

### Task 4: Rewrite `Route` to Use `BoxProcessor` Pipeline

**Files:**
- Modify: `crates/camel-core/src/route.rs` (replace entire file)
- Modify: `crates/camel-core/src/lib.rs` (update re-exports)

**Step 1: Rewrite route.rs**

Replace `crates/camel-core/src/route.rs` entirely:

```rust
use camel_api::BoxProcessor;

/// A Route defines a message flow: from a source endpoint, through a composed
/// Tower Service pipeline.
pub struct Route {
    /// The source endpoint URI.
    pub(crate) from_uri: String,
    /// The composed processor pipeline as a type-erased Tower Service.
    pub(crate) pipeline: BoxProcessor,
}

impl Route {
    /// Create a new route from the given source URI and processor pipeline.
    pub fn new(from_uri: impl Into<String>, pipeline: BoxProcessor) -> Self {
        Self {
            from_uri: from_uri.into(),
            pipeline,
        }
    }

    /// The source endpoint URI.
    pub fn from_uri(&self) -> &str {
        &self.from_uri
    }

    /// Consume the route and return its pipeline.
    pub fn into_pipeline(self) -> BoxProcessor {
        self.pipeline
    }
}
```

**Step 2: Update lib.rs re-exports**

Change `crates/camel-core/src/lib.rs`:

```rust
pub mod context;
pub mod registry;
pub mod route;

pub use context::CamelContext;
pub use registry::Registry;
pub use route::Route;
```

Remove the old `ProcessorFn` and `RouteStep` re-exports.

**Step 3: Run check (not test yet — context.rs and builder will break)**

Run: `cargo check -p camel-core --lib`
Expected: Compilation errors in `context.rs` referencing old `RouteStep`. This is expected — we fix context next.

**Step 4: Commit (WIP)**

```
refactor(core): replace ProcessorFn/RouteStep with BoxProcessor pipeline in Route

WIP: context.rs needs updating to match new Route structure
```

---

### Task 5: Rewrite `CamelContext` to Execute Tower Pipelines

**Files:**
- Modify: `crates/camel-core/src/context.rs` (replace pipeline execution)

**Step 1: Rewrite context.rs**

Replace `crates/camel-core/src/context.rs`:

```rust
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tower::ServiceExt;
use tracing::{error, info};

use camel_api::{CamelError, Exchange};
use camel_component::{Component, ConsumerContext};
use camel_endpoint::parse_uri;

use crate::registry::Registry;
use crate::route::Route;

/// The CamelContext is the runtime engine that manages components, routes, and their lifecycle.
pub struct CamelContext {
    registry: Registry,
    routes: Vec<Route>,
    tasks: Vec<JoinHandle<()>>,
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl CamelContext {
    /// Create a new, empty CamelContext.
    pub fn new() -> Self {
        Self {
            registry: Registry::new(),
            routes: Vec::new(),
            tasks: Vec::new(),
            shutdown_tx: None,
        }
    }

    /// Register a component with this context.
    pub fn register_component<C: Component + 'static>(&mut self, component: C) {
        info!(scheme = component.scheme(), "Registering component");
        self.registry.register(component);
    }

    /// Add a route to this context.
    pub fn add_route(&mut self, route: Route) {
        info!(from = route.from_uri(), "Adding route");
        self.routes.push(route);
    }

    /// Access the component registry.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Start all routes. Each route's consumer will begin producing exchanges.
    pub async fn start(&mut self) -> Result<(), CamelError> {
        info!("Starting CamelContext with {} routes", self.routes.len());

        let (shutdown_tx, _shutdown_rx) = mpsc::channel::<()>(1);
        self.shutdown_tx = Some(shutdown_tx.clone());

        // Take routes to iterate
        let routes = std::mem::take(&mut self.routes);

        for route in routes {
            let from_uri = route.from_uri().to_string();
            let from = parse_uri(&from_uri)?;
            let component = self.registry.get_or_err(&from.scheme)?;
            let endpoint = component.create_endpoint(&from_uri)?;

            // Create a channel for the consumer to send exchanges into
            let (tx, mut rx) = mpsc::channel::<Exchange>(256);
            let consumer_ctx = ConsumerContext::new(tx);

            // Start the consumer in a background task
            let mut consumer = endpoint.create_consumer()?;
            let consumer_handle = tokio::spawn(async move {
                if let Err(e) = consumer.start(consumer_ctx).await {
                    error!("Consumer error: {e}");
                }
            });
            self.tasks.push(consumer_handle);

            // Get the pipeline from the route
            let mut pipeline = route.into_pipeline();
            let shutdown_tx_clone = shutdown_tx.clone();

            // Spawn a task that reads exchanges and runs them through the Tower pipeline
            let pipeline_handle = tokio::spawn(async move {
                let _shutdown = shutdown_tx_clone;

                while let Some(exchange) = rx.recv().await {
                    // Ensure the service is ready before calling
                    match pipeline.ready().await {
                        Ok(svc) => {
                            if let Err(e) = svc.call(exchange).await {
                                error!("Pipeline error: {e}");
                            }
                        }
                        Err(e) => {
                            error!("Pipeline not ready: {e}");
                            break;
                        }
                    }
                }
            });
            self.tasks.push(pipeline_handle);
        }

        info!("CamelContext started");
        Ok(())
    }

    /// Stop the context, shutting down all routes.
    pub async fn stop(&mut self) -> Result<(), CamelError> {
        info!("Stopping CamelContext");

        // Drop shutdown sender to signal tasks
        self.shutdown_tx.take();

        // Abort all tasks
        for task in self.tasks.drain(..) {
            task.abort();
        }

        info!("CamelContext stopped");
        Ok(())
    }
}

impl Default for CamelContext {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 2: Run check**

Run: `cargo check -p camel-core --lib`
Expected: Compiles. (Tests and builder will still be broken.)

**Step 3: Commit (WIP)**

```
refactor(core): rewrite CamelContext to execute Tower pipelines via Service::call
```

---

### Task 6: Rewrite `RouteBuilder` to Compose Tower Services

**Files:**
- Modify: `crates/camel-builder/src/lib.rs` (complete rewrite)
- Modify: `crates/camel-builder/Cargo.toml` (add tower dependency)

**Step 1: Update Cargo.toml**

In `crates/camel-builder/Cargo.toml`, add:

```toml
tower.workspace = true
```

**Step 2: Rewrite the builder**

Replace `crates/camel-builder/src/lib.rs` with a new implementation. The builder accumulates pipeline steps and composes them into a `BoxProcessor` on `.build()`.

Key design decisions:
- Steps are stored as an enum `BuilderStep` (process, filter, set_header, map_body, to)
- On `.build()`, steps are composed right-to-left: the last step wraps `IdentityProcessor`, then each earlier step wraps the result
- `.to()` steps store a URI string — resolved at start time to a `BoxProcessor` via `Endpoint::create_producer()`

**Important consideration:** The builder cannot resolve URIs (it doesn't have the registry). Two options:
1. The builder stores URI strings, and the context resolves them during `start()`.
2. The builder receives a reference to the registry.

The cleanest approach: the builder produces an intermediate representation, and `CamelContext::start()` or a `RouteBuilder::build_with_context()` resolves URIs. However, this adds complexity. A simpler approach for now: the route stores both the pipeline and the list of "to" URIs, and the context resolves producers (which are now `BoxProcessor` from `Endpoint::create_producer()`) into the pipeline at start time.

**Actually, the simplest correct approach:** The builder stores steps. `build()` produces a `RouteDefinition` with unresolved URIs. `CamelContext::add_route()` or `CamelContext::start()` resolves URIs via `Endpoint::create_producer()` (which returns `BoxProcessor`) and creates the final composed pipeline. This means we keep a `RouteDefinition` (builder output) and `Route` (resolved, with pipeline). The context converts definitions into routes.

```rust
use camel_api::body::Body;
use camel_api::{BoxProcessor, CamelError, Exchange, IdentityProcessor, ProcessorFn, Value};
use camel_processor::{Filter, MapBody, SetHeader};

/// A step in the route definition (unresolved — URIs not yet looked up).
enum BuilderStep {
    /// A Tower processor service, boxed for type erasure.
    Processor(BoxProcessor),
    /// A destination URI — resolved at start time by CamelContext.
    To(String),
}

/// A fluent builder for constructing routes.
///
/// # Example
///
/// ```ignore
/// let route = RouteBuilder::from("timer:tick?period=1000")
///     .set_header("source", Value::String("timer".into()))
///     .filter(|ex| ex.input.body.as_text().is_some())
///     .to("log:info")
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

    /// Add a filter step. Exchanges that do **not** match the predicate
    /// skip the rest of the pipeline (real gate semantics).
    pub fn filter<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&Exchange) -> bool + Clone + Send + Sync + 'static,
    {
        let svc = Filter::new(IdentityProcessor, predicate);
        self.steps
            .push(BuilderStep::Processor(BoxProcessor::new(svc)));
        self
    }

    /// Add a processor step defined by an async closure.
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
    /// endpoint URI.
    pub fn to(mut self, endpoint: &str) -> Self {
        self.steps
            .push(BuilderStep::To(endpoint.to_string()));
        self
    }

    /// Consume the builder and produce a [`RouteDefinition`].
    pub fn build(self) -> Result<RouteDefinition, CamelError> {
        if self.from_uri.is_empty() {
            return Err(CamelError::RouteError(
                "route must have a 'from' URI".to_string(),
            ));
        }

        Ok(RouteDefinition {
            from_uri: self.from_uri,
            steps: self.steps,
        })
    }
}

/// An unresolved route definition — "to" URIs have not been resolved to producers yet.
/// Call `CamelContext::add_route()` to resolve and register.
pub struct RouteDefinition {
    pub(crate) from_uri: String,
    pub(crate) steps: Vec<BuilderStep>,
}

impl RouteDefinition {
    pub fn from_uri(&self) -> &str {
        &self.from_uri
    }

    /// Resolve all "to" URIs using the given registry and compose the final pipeline.
    pub(crate) fn resolve(
        self,
        registry: &camel_core::Registry,
    ) -> Result<camel_core::Route, CamelError> {
        use camel_endpoint::parse_uri;
        use tower::ServiceExt;

        let mut processors: Vec<BoxProcessor> = Vec::new();

        for step in self.steps {
            match step {
                BuilderStep::Processor(svc) => {
                    processors.push(svc);
                }
                BuilderStep::To(uri) => {
                    let parsed = parse_uri(&uri)?;
                    let component = registry.get_or_err(&parsed.scheme)?;
                    let endpoint = component.create_endpoint(&uri)?;
                    let producer = endpoint.create_producer()?;
                    processors.push(producer);
                }
            }
        }

        // Compose all processors into a single pipeline.
        // Each processor is called sequentially: output of one feeds into the next.
        let pipeline = compose_pipeline(processors);

        Ok(camel_core::Route::new(self.from_uri, pipeline))
    }
}

/// Compose a list of BoxProcessors into a single pipeline that runs them sequentially.
fn compose_pipeline(processors: Vec<BoxProcessor>) -> BoxProcessor {
    if processors.is_empty() {
        return BoxProcessor::new(IdentityProcessor);
    }

    // Create a sequential pipeline: each processor feeds into the next.
    // We build a ChainService that calls each processor in order.
    let pipeline = SequentialPipeline { steps: processors };
    BoxProcessor::new(pipeline)
}

/// A service that executes a sequence of BoxProcessors in order.
#[derive(Clone)]
struct SequentialPipeline {
    steps: Vec<BoxProcessor>,
}

impl tower::Service<Exchange> for SequentialPipeline {
    type Response = Exchange;
    type Error = CamelError;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let mut steps = self.steps.clone();
        Box::pin(async move {
            let mut ex = exchange;
            for step in &mut steps {
                ex = step.ready().await?.call(ex).await?;
            }
            Ok(ex)
        })
    }
}
```

**Step 3: Update CamelContext to accept RouteDefinition**

Modify `crates/camel-core/src/context.rs` to add:

```rust
/// Add a route definition. The definition's "to" URIs are resolved immediately
/// using the registered components.
pub fn add_route_definition(&mut self, definition: camel_builder::RouteDefinition) -> Result<(), CamelError> {
    let route = definition.resolve(&self.registry)?;
    self.add_route(route);
    Ok(())
}
```

Wait — this creates a circular dependency: `camel-core` depending on `camel-builder`. Instead, move resolution logic to `camel-core`, or have the context accept both `Route` and a builder-produced intermediate.

**Better approach:** Keep `RouteDefinition` in `camel-core` (not in `camel-builder`). The builder uses `camel-core::RouteDefinition`. The context resolves definitions.

Actually, let's keep it simpler. `CamelContext::add_route()` already takes a `Route` with a resolved pipeline. We just need `RouteBuilder::build_with_registry()`:

```rust
pub fn build(self, registry: &camel_core::Registry) -> Result<camel_core::Route, CamelError> {
    // resolve URIs and compose pipeline here
}
```

But that requires the user to pass the registry. Alternatively, `CamelContext` could have a `build_route` method.

**Simplest correct approach:** The builder does NOT resolve URIs. It produces a `RouteDefinition` (owned by `camel-builder`). The `CamelContext::add_route()` method accepts `RouteDefinition` and resolves internally. To avoid circular deps, we define a `RouteDefinition` trait or struct in `camel-core`.

**Final decision:** Put `RouteDefinition` and `BuilderStep` in `camel-core`. The builder depends on `camel-core` (already does). The context resolves definitions.

Revised plan:
- `camel-core::route` exports `Route`, `RouteDefinition`, `BuilderStep`
- `camel-builder` creates `RouteDefinition`
- `CamelContext::add_route()` accepts `RouteDefinition`, resolves, stores `Route`

**Step 4: Run tests**

Run: `cargo test -p camel-builder`
Expected: All builder tests pass (may need updating for new API).

**Step 5: Commit**

```
refactor(builder): rewrite RouteBuilder to compose real Tower Services
```

---

### Task 7: Rewrite `MockComponent` with Shared Registry

**Files:**
- Modify: `crates/components/camel-mock/src/lib.rs`

**Step 1: Write test for shared registry**

Add to mock tests:

```rust
#[tokio::test]
async fn test_mock_component_shared_registry() {
    let component = MockComponent::new();

    // Two create_endpoint calls with the same name should return endpoints
    // that share the same received exchanges storage.
    let _ep1 = component.create_endpoint("mock:shared").unwrap();
    let ep2 = component.create_endpoint("mock:shared").unwrap();

    let producer = ep2.create_producer().unwrap();
    producer
        .process(Exchange::new(Message::new("test")))
        .await
        .unwrap();

    // Access via the component's get_endpoint method
    let endpoint = component.get_endpoint("shared").unwrap();
    endpoint.assert_exchange_count(1).await;
}
```

**Step 2: Run test — should fail**

Run: `cargo test -p camel-mock -- test_mock_component_shared_registry`
Expected: FAIL — `get_endpoint` doesn't exist.

**Step 3: Implement shared registry**

Rewrite `MockComponent` to use a shared registry:

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct MockComponent {
    registry: Arc<Mutex<HashMap<String, Arc<MockEndpoint>>>>,
}

impl MockComponent {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Retrieve a mock endpoint by name. Returns `None` if it hasn't been created yet.
    pub async fn get_endpoint(&self, name: &str) -> Option<Arc<MockEndpoint>> {
        self.registry.lock().await.get(name).cloned()
    }
}
```

Update `create_endpoint` to use get-or-create pattern:

```rust
fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
    let parts = parse_uri(uri)?;
    // ... validation ...

    // Blocking lock is needed here since Component::create_endpoint is not async.
    // Use try_lock or restructure if this becomes a problem.
    let rt = tokio::runtime::Handle::current();
    let registry = self.registry.clone();
    let name = parts.path.clone();
    let uri_str = uri.to_string();

    // Since create_endpoint is sync, we need to block
    let endpoint = rt.block_in_place(|| {
        rt.block_on(async {
            let mut reg = registry.lock().await;
            if let Some(existing) = reg.get(&name) {
                existing.clone()
            } else {
                let ep = Arc::new(MockEndpoint {
                    uri: uri_str,
                    _name: name.clone(),
                    received: Arc::new(Mutex::new(Vec::new())),
                });
                reg.insert(name, ep.clone());
                ep
            }
        })
    });

    Ok(Box::new(MockEndpointRef(endpoint)))
}
```

Wait — `Component::create_endpoint` is synchronous. Using `block_in_place` inside async context is fragile. Better approach: use a `std::sync::Mutex` for the registry instead of `tokio::sync::Mutex`.

```rust
use std::sync::Mutex as StdMutex;

pub struct MockComponent {
    registry: Arc<StdMutex<HashMap<String, Arc<MockEndpoint>>>>,
}

impl MockComponent {
    pub fn get_endpoint(&self, name: &str) -> Option<Arc<MockEndpoint>> {
        self.registry.lock().unwrap().get(name).cloned()
    }
}

impl Component for MockComponent {
    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        let parts = parse_uri(uri)?;
        // ... validation ...

        let mut reg = self.registry.lock().unwrap();
        let endpoint = reg
            .entry(parts.path.clone())
            .or_insert_with(|| {
                Arc::new(MockEndpoint {
                    uri: uri.to_string(),
                    _name: parts.path.clone(),
                    received: Arc::new(Mutex::new(Vec::new())),
                })
            })
            .clone();

        Ok(Box::new(MockEndpointRef(endpoint)))
    }
}
```

We need a `MockEndpointRef` wrapper that implements `Endpoint` and delegates to the shared `Arc<MockEndpoint>`.

**Step 4: Update all existing tests**

Existing unit tests that construct `MockEndpoint` directly need updating to use the new API.

**Step 5: Run all tests**

Run: `cargo test -p camel-mock`
Expected: All tests pass.

**Step 6: Commit**

```
feat(mock): implement shared endpoint registry for MockComponent
```

---

### Task 8: Create `camel-test` Crate and Migrate Integration Tests

**Files:**
- Create: `crates/camel-test/Cargo.toml`
- Create: `crates/camel-test/src/lib.rs`
- Move: `crates/camel-core/tests/integration_test.rs` -> `crates/camel-test/tests/integration_test.rs`
- Modify: `Cargo.toml` (root, add workspace member)
- Modify: `crates/camel-core/Cargo.toml` (remove dev-dependencies)

**Step 1: Create the crate**

Create `crates/camel-test/Cargo.toml`:

```toml
[package]
name = "camel-test"
description = "Testing utilities and integration tests for rust-camel"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
camel-api.workspace = true
camel-core.workspace = true
camel-builder.workspace = true
camel-component.workspace = true
camel-processor.workspace = true
camel-mock.workspace = true
camel-timer.workspace = true
camel-log.workspace = true
camel-direct.workspace = true
tokio.workspace = true
tower.workspace = true

[dev-dependencies]
serde_json.workspace = true
```

Create `crates/camel-test/src/lib.rs`:

```rust
//! Testing utilities for rust-camel.
//!
//! This crate provides helpers for writing integration tests against the
//! rust-camel framework. It re-exports commonly needed types and provides
//! test-specific utilities.

pub use camel_mock::MockComponent;
```

**Step 2: Add to workspace**

In root `Cargo.toml`, add `"crates/camel-test"` to workspace members.

**Step 3: Migrate integration tests**

Move `crates/camel-core/tests/integration_test.rs` to `crates/camel-test/tests/integration_test.rs`.

Update imports to use the new API (RouteDefinition, Tower-based pipeline, MockComponent with shared registry).

**Step 4: Clean up camel-core dev-dependencies**

Remove from `crates/camel-core/Cargo.toml` `[dev-dependencies]`:
- `camel-builder`
- `camel-timer`
- `camel-log`
- `camel-mock`
- `serde_json`

**Step 5: Rewrite integration tests to use MockComponent**

Replace the `capture_step` workaround with proper `MockComponent` usage:

```rust
#[tokio::test]
async fn test_timer_to_mock() {
    let mock = MockComponent::new();

    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone()); // MockComponent must be Clone

    let route = RouteBuilder::from("timer:tick?period=50&repeatCount=3")
        .to("mock:result")
        .build()
        .unwrap();

    ctx.add_route(route);
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ctx.stop().await.unwrap();

    let endpoint = mock.get_endpoint("result").unwrap();
    endpoint.assert_exchange_count(3).await;
}
```

**Step 6: Run all tests**

Run: `cargo test -p camel-test`
Expected: All integration tests pass.

**Step 7: Commit**

```
refactor(test): create camel-test crate and migrate integration tests from camel-core
```

---

### Task 9: Update All Examples

**Files:**
- Modify: `examples/hello-world/src/main.rs`
- Modify: `examples/content-based-routing/src/main.rs`
- Modify: `examples/multi-route-direct/src/main.rs`
- Modify: `examples/transform-pipeline/src/main.rs`
- Modify: `examples/showcase/src/main.rs`

**Step 1: Read each example and update API calls**

The main changes:
- `RouteBuilder::build()` may now return `RouteDefinition` instead of `Route`
- `ctx.add_route()` may need to become `ctx.add_route_definition()` or stay the same
- Filter semantics changed (real gate, not property marking)

Update each example to match the new API.

**Step 2: Build all examples**

Run: `cargo build --examples` or `cargo build -p hello-world -p content-based-routing -p multi-route-direct -p transform-pipeline -p showcase`
Expected: All compile.

**Step 3: Commit**

```
refactor(examples): update all examples for new Tower-based API
```

---

### Task 10: Full Test Suite, Clippy, Fmt

**Files:** All

**Step 1: Run full test suite**

Run: `cargo test --workspace`
Expected: All tests pass.

**Step 2: Run clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: No warnings.

**Step 3: Run fmt**

Run: `cargo fmt --all -- --check`
Expected: Clean.

**Step 4: Fix any issues found**

Address all failures iteratively.

**Step 5: Commit**

```
chore: clean up after Tower reconnection — clippy, fmt, test fixes
```

---

### Task 11: Update Documentation

**Files:**
- Modify: `docs/plans/2026-02-28-rust-camel-core-design.md` (update Implementation Notes)
- Modify: `docs/plans/2026-02-28-rust-camel-core-implementation.md` (mark deviations as resolved)

**Step 1: Update design doc**

Add to the Implementation Notes section that all 7 deviations have been resolved:
- Tower reconnected in runtime
- Layer types added
- Producers are Tower-native (no adapter needed)
- Filter has real gate semantics
- MockComponent has shared registry
- Integration tests in camel-test crate
- camel-processor is live runtime dependency

**Step 2: Commit**

```
docs: update design and implementation docs after Tower reconnection
```

---

## Summary of Tasks

| Task | Description | Depends On | Status | Commit |
|------|-------------|------------|--------|--------|
| 1 | BoxProcessor + ProcessorFn in camel-api | — | Done | `0dea0b1` |
| 2 | Layer types in camel-processor | — | Done | `77d0fb8` |
| 3 | Tower-native producers (remove Producer trait) | 1 | Done | `0378de6`, `cb7a361` |
| 4 | Rewrite Route with BoxProcessor | 1 | Done | `6ed39a0` |
| 5 | Rewrite CamelContext for Tower execution | 3, 4 | Done | `a9ad3dd` |
| 6 | Rewrite RouteBuilder with Tower composition | 1, 2, 3, 4 | Done | `aa96686` |
| 7 | MockComponent shared registry | 3 | Done | `c3f98b7`, `8730648` |
| 8 | Create camel-test, migrate integration tests | 5, 6, 7 | Done | `842bdaa` |
| 9 | Update all examples | 6 | Done | `2727f53` |
| 10 | Full test suite + clippy + fmt | 8, 9 | Done | `dca7cb6` |
| 11 | Update documentation | 10 | Done | `06b16dd`, `0e99c91` |

Extra commits: `81052fd` (integration test fix between Tasks 6-7).

Tasks 1 and 2 are independent and can be done in parallel. Task 3 depends on 1 (needs BoxProcessor). Tasks 4 and 3 can be parallel. Task 5 depends on 3 and 4. Task 6 depends on 1, 2, 3, 4. Task 7 depends on 3 (new Endpoint trait). Task 8 depends on 5, 6, 7.

**All 11 tasks completed.** Date completed: 2026-02-28.

---

## Implementation Deviations

### Task 1: Manual `Clone` impl on `ProcessorFn`

Plan specified `#[derive(Clone)]`. Implementation uses a manual `Clone` impl because `#[derive(Clone)]` adds a spurious `F: Clone` bound even though `Arc<F>` is always `Clone`. Quality improvement.

### Task 2: Direct struct construction in Layer impls

Plan specified `Filter::new(inner, predicate)` in `FilterLayer::layer()`. Implementation uses direct struct field construction (`Filter { inner, predicate: ... }`) for `FilterLayer` and `MapBodyLayer` to avoid over-constraining `Fn` bounds. `SetHeaderLayer` uses `::new()` as planned. Functionally identical.

### Tasks 4/5/6: Structural reorganization — types in `camel-core`, not `camel-builder`

The plan initially placed `RouteDefinition`, `BuilderStep`, `SequentialPipeline`, and `compose_pipeline` in `camel-builder`, then revised to place them in `camel-core` (to avoid circular dependencies). In practice:

- `RouteDefinition`, `BuilderStep`, `SequentialPipeline`, `compose_pipeline` were placed in `camel-core::route` during **Task 4** (not Task 6).
- `CamelContext::add_route_definition()` was added during **Task 5** (not mentioned in plan). This method resolves `BuilderStep::To` URIs eagerly via the registry — replacing the plan's `RouteDefinition::resolve()` method that was never implemented.
- `RouteBuilder` (Task 6) became a thin fluent wrapper that imports types from `camel-core::route`.

This is the most significant deviation. The plan's own "Final decision" narrative anticipated this reorganization, but it was executed across Tasks 4-5 rather than in Task 6.

### Task 3: `producer.rs` deletion in separate commit; `camel-timer` not mentioned

Plan said to delete `producer.rs` as part of Task 3. The file was left orphaned and deleted in a follow-up commit (`cb7a361`) after code review. Additionally, `camel-timer/src/lib.rs` needed updating (`create_producer()` return type → `BoxProcessor`) which was not listed in the plan's Task 3 file list.

### Task 5: No `Arc` import; eager resolution

Plan included `use std::sync::Arc;` — not needed in the actual implementation. URI resolution happens eagerly in `add_route_definition()` rather than lazily at `start()` time.

### Task 7: Naming differences and mutex strategy

- Plan used `MockEndpointRef` wrapper — implementation uses `MockEndpoint(Arc<MockEndpointInner>)` (different naming, same pattern).
- `get_endpoint()` returns `Option<Arc<MockEndpointInner>>` (inner type for assertions), not `Option<Arc<MockEndpoint>>`.
- Registry uses `std::sync::Mutex` (as plan concluded). Received exchanges storage uses `tokio::sync::Mutex` (plan didn't distinguish these two).

### Extra commits (4 total)

| Commit | Description | Reason |
|--------|-------------|--------|
| `cb7a361` | Delete orphaned `producer.rs` | Missed in Task 3, caught in review |
| `81052fd` | Fix integration tests for RouteDefinition API | Needed between Tasks 6-7 |
| `8730648` | Fix mock encapsulation, propagate mutex error | Post-Task 7 review feedback |
| `0e99c91` | Fix doc gaps, ProcessorFn phrasing | Post-Task 11 review feedback |

15 total commits vs 11 planned tasks. All 4 extra commits are review-driven fixes — evidence of the two-stage review process working as intended.
