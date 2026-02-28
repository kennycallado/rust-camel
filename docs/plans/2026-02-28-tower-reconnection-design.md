# Tower Reconnection Design

## Context

The rust-camel MVP implementation (13 tasks, 79 tests passing) has a fundamental architectural deviation: Tower Service types exist in `camel-api` and `camel-processor` but the runtime (`camel-core`, `camel-builder`) bypasses them entirely, using flat async closures instead.

This document describes the design for reconnecting the runtime with Tower and resolving all 7 identified architectural deviations.

## Problem Statement

Three parallel, disconnected systems exist for processing exchanges:

| System | Mechanism | Tower? | Used at runtime? |
|---|---|---|---|
| `camel-api::Processor` | `Service<Exchange>` blanket impl | Yes | No |
| `camel-processor::Filter/MapBody/SetHeader` | `impl Service<Exchange>` wrapping inner | Yes | No |
| `camel-core::ProcessorFn` + `RouteStep` | `Arc<dyn Fn(Exchange) -> Pin<Box<Future>>>` | No | Yes |
| `camel-component::Producer` | `async_trait` with `process(&self, Exchange)` | No | Yes |
| `camel-builder::RouteBuilder` | Inline closures creating `ProcessorFn` | No | Yes |

This creates dead code (`camel-processor` crate), duplicated logic (builder reimplements filter/set_header/map_body as closures), and semantic bugs (builder's filter marks a property but does not stop the pipeline).

## Deviations Addressed

| # | Deviation | Resolution |
|---|---|---|
| 1 | Tower used in types/tests but runtime bypasses it | Runtime executes real `Service<Exchange>` chains |
| 2 | `ProcessorExt` / composition never implemented | Layer types + ServiceBuilder composition |
| 3 | Producer is `async_trait` not Tower Service | Producer becomes Tower-native: `Endpoint::create_producer()` returns `BoxProcessor` |
| 4 | `.filter()` marks property but does not drop exchanges | Real gate semantics: non-matching exchanges skip the rest of the pipeline |
| 5 | Integration tests in `camel-core/tests/` | Migrate to `crates/camel-test/tests/` |
| 6 | `MockComponent` unusable in e2e tests | Shared registry pattern + `CamelContext::get_mock_endpoint()` |
| 7 | `camel-processor` is dead runtime dependency | Builder composes real Tower Service types |

## Design

### 1. Route Execution Model

**Before (flat dispatcher):**

```
Consumer -> mpsc channel -> for step in Vec<RouteStep> { match step { Process(fn) | To(uri) } }
```

**After (Tower pipeline):**

```
Consumer -> mpsc channel -> service_chain.call(exchange).await
```

Where `service_chain` is a composed Tower Service built by the builder:

```
Filter<SetHeader<MapBody<ProducerService<IdentityProcessor>>>>
```

The entire route pipeline is a single `impl Service<Exchange>` that the context calls once per exchange.

### 2. Type-Erased Pipeline

Since routes are built dynamically, the concrete Service type varies per route. We use type erasure:

```rust
// In camel-api or camel-core
pub type BoxProcessor = tower::util::BoxCloneService<Exchange, Exchange, CamelError>;
```

The builder composes real Tower Services then erases the final type into `BoxProcessor`. The context stores and calls `BoxProcessor`.

### 3. Route Representation

```rust
// In camel-core::route
pub struct Route {
    pub(crate) from_uri: String,
    pub(crate) pipeline: BoxProcessor,
}
```

`ProcessorFn`, `RouteStep` enum, and the parallel `Vec<Option<Arc<dyn Producer>>>` are eliminated.

### 4. Layer Types in camel-processor

Each processor gets a corresponding Layer for ServiceBuilder composition:

```rust
// Example: FilterLayer
pub struct FilterLayer<F> { predicate: F }

impl<S, F> Layer<S> for FilterLayer<F>
where
    S: Service<Exchange> + Clone,
    F: Fn(&Exchange) -> bool + Clone,
{
    type Service = Filter<S, F>;
    fn layer(&self, inner: S) -> Self::Service {
        Filter::new(inner, self.predicate.clone())
    }
}
```

Same pattern for `SetHeaderLayer`, `MapBodyLayer`.

### 5. Filter Semantics

The `Filter` Service already has correct semantics: if predicate returns false, the exchange is returned as-is without calling the inner service. This means subsequent processors in the chain are skipped for non-matching exchanges.

This matches Apache Camel's filter behavior where `.filter()` acts as a gate.

### 6. Tower-Native Producers (No Adapter)

The `Producer` async_trait is eliminated entirely. Producers become Tower-native:

```rust
// camel-component::endpoint (updated)
pub trait Endpoint: Send + Sync {
    fn uri(&self) -> &str;
    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError>;
    fn create_producer(&self) -> Result<BoxProcessor, CamelError>;
}
```

Each component implements `Service<Exchange>` directly on its producer type:

```rust
// Example: MockProducer
#[derive(Clone)]
struct MockProducer {
    received: Arc<Mutex<Vec<Exchange>>>,
}

impl Service<Exchange> for MockProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let received = Arc::clone(&self.received);
        Box::pin(async move {
            received.lock().await.push(exchange.clone());
            Ok(exchange)
        })
    }
}
```

The endpoint returns the producer as a `BoxProcessor`:

```rust
fn create_producer(&self) -> Result<BoxProcessor, CamelError> {
    Ok(BoxProcessor::new(MockProducer { received: Arc::clone(&self.received) }))
}
```

This eliminates the `ProducerService` adapter entirely. Producers and processors are the same abstraction: `Service<Exchange>`. The distinction is purely semantic.

**Consumer stays as `async_trait`** — it is a long-lived loop (`start`/`stop`), not a request→response transform. It does not fit the Tower model.

### 7. Builder Rewrite

The `RouteBuilder` stops creating inline closures and instead composes real Tower Services.

The builder accumulates layers and builds the final Service chain when `.build()` is called:

```rust
impl RouteBuilder {
    pub fn filter<F>(self, predicate: F) -> Self { /* store FilterLayer */ }
    pub fn set_header(self, key: impl Into<String>, value: impl Into<Value>) -> Self { /* store SetHeaderLayer */ }
    pub fn map_body<F>(self, mapper: F) -> Self { /* store MapBodyLayer */ }
    pub fn to(self, uri: &str) -> Self { /* store ToStep */ }
    pub fn process<P: Processor>(self, processor: P) -> Self { /* store processor */ }
    pub fn build(self) -> Route { /* compose all layers into BoxProcessor */ }
}
```

API may change from current version if it improves DX, stability, or composability.

### 8. CamelContext Execution

```rust
// In context.rs, the pipeline loop becomes:
while let Some(exchange) = rx.recv().await {
    match pipeline.call(exchange).await {
        Ok(_) => {}
        Err(e) => error!("Pipeline error: {e}"),
    }
}
```

Single `call` replaces the flat `for` loop over steps. Error handling is natural through the Service chain.

### 9. MockComponent with Shared Registry

Following the `DirectComponent` pattern:

```rust
pub struct MockComponent {
    registry: Arc<Mutex<HashMap<String, Arc<MockEndpoint>>>>,
}

impl MockComponent {
    pub fn new() -> Self { /* shared registry */ }
    pub fn get_endpoint(&self, name: &str) -> Option<Arc<MockEndpoint>> { /* lookup */ }
}

impl Component for MockComponent {
    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        // Parse name from URI, get-or-create in registry
        // Return the SAME endpoint for the same name
    }
}
```

Exposed through CamelContext:

```rust
impl CamelContext {
    pub fn get_mock_endpoint(&self, name: &str) -> Option<Arc<MockEndpoint>>
}
```

### 10. camel-test Crate

New crate at `crates/camel-test/`:

- **Purpose:** Test harness, utilities, and integration tests
- **Dependencies:** `camel-core`, `camel-builder`, `camel-mock`, `camel-timer`, `camel-log`, `camel-direct` (all as regular deps since this is a test-only crate)
- **Contents:**
  - Test helpers (replacement for `capture_step` workaround)
  - Re-exports of `MockComponent` and testing utilities
  - Integration tests in `tests/` directory
- **Workspace role:** Only used as a dev-dependency by other crates, or run directly via `cargo test -p camel-test`

Integration tests currently in `crates/camel-core/tests/integration_test.rs` are migrated here. The dev-dependencies on `camel-builder`, `camel-timer`, `camel-log`, `camel-mock` are removed from `camel-core`.

## Crates Affected

| Crate | Change |
|---|---|
| `camel-api` | Add `BoxProcessor` type alias |
| `camel-processor` | Add `FilterLayer`, `SetHeaderLayer`, `MapBodyLayer`; review/adjust Service impls |
| `camel-core` | Rewrite `route.rs` (new `Route` with `BoxProcessor`), rewrite `context.rs` (Tower pipeline execution), add `get_mock_endpoint()` |
| `camel-component` | Remove `Producer` trait, update `Endpoint::create_producer()` to return `BoxProcessor` |
| `camel-builder` | Rewrite to compose real Tower Services instead of inline closures |
| `camel-mock` | Add shared registry pattern |
| `camel-test` | **New** — test harness + migrated integration tests |
| Examples | Update to match any API changes |

## Non-Goals

- Graceful shutdown / drain (future work)
- Backpressure propagation (Tower supports it, but we keep `poll_ready` as always-ready for now)
- EIP patterns beyond filter/map_body/set_header (Splitter, Aggregator, etc. are future work but this architecture enables them)
- Breaking the `Component`/`Endpoint`/`Consumer` trait hierarchy (`Component` and `Consumer` stay as `async_trait`; `Producer` is replaced by Tower-native `BoxProcessor`)
