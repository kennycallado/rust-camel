# Pipeline Concurrency — Design

> Approved 2026-03-01. Supersedes `pipeline-concurrency-analysis.md`.

## Problem

`CamelContext::start()` runs one pipeline task per route that reads from `mpsc::channel(256)` sequentially. Components that accept inbound connections (HTTP server, WebSocket, Kafka) produce concurrent exchanges, but request #2 waits until request #1 finishes the entire pipeline. At 50ms per exchange, 100 concurrent HTTP requests means the last one waits ~5 seconds.

## Decision

**F+C combined: smart defaults with user override.**

The consumer declares its concurrency model via a new trait method. The runtime acts on it automatically. The user can override per-route via the builder DSL.

### Eliminated options

| Option | Reason |
|--------|--------|
| A — spawn per-exchange globally | Makes everything concurrent; breaks ordering invariants for timer/file |
| B — `tower::Buffer` | Error propagation semantics conflict with existing ErrorHandler/DLC |
| D — consumer bypasses channel | Major interface change; breaks consumer/pipeline decoupling |
| E — typestate on RouteBuilder | 100% of routes use URI strings; no types to propagate |
| C pure — explicit only | Forgetting `.concurrent()` on an HTTP route is a silent performance bug |
| F pure — consumer only | No user override; inflexible for power users |

## Design

### 1. ConcurrencyModel enum and Consumer trait extension

```rust
// camel-component/src/consumer.rs

#[derive(Debug, Clone, PartialEq)]
pub enum ConcurrencyModel {
    Sequential,
    Concurrent { max: Option<usize> },
}

#[async_trait]
pub trait Consumer: Send + Sync {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError>;
    async fn stop(&mut self) -> Result<(), CamelError>;

    /// Declares this consumer's natural concurrency model.
    /// Default: Sequential (correct for timer, file, direct).
    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }
}
```

Consumer implementations:

```rust
// HttpConsumer — override
fn concurrency_model(&self) -> ConcurrencyModel {
    ConcurrencyModel::Concurrent { max: None }
}

// TimerConsumer, FileConsumer, DirectConsumer — no override needed
```

### 2. RouteDefinition: new field

```rust
// camel-core/src/route.rs

pub struct RouteDefinition {
    pub(crate) from_uri: String,
    pub(crate) steps: Vec<BuilderStep>,
    pub(crate) error_handler: Option<ErrorHandlerConfig>,
    pub(crate) circuit_breaker: Option<CircuitBreakerConfig>,
    pub(crate) concurrency: Option<ConcurrencyModel>,  // user override
}
```

`None` means "use whatever the consumer declares". `Some(...)` overrides the consumer's default.

### 3. RouteBuilder DSL

```rust
// camel-builder/src/lib.rs

impl RouteBuilder {
    /// Override the consumer's default concurrency model.
    /// `.concurrent(16)` — spawn per-exchange, semaphore-limited to 16.
    /// `.concurrent(0)` — spawn per-exchange, unbounded (channel buffer is backpressure).
    pub fn concurrent(mut self, max: usize) -> Self {
        let max = if max == 0 { None } else { Some(max) };
        self.concurrency = Some(ConcurrencyModel::Concurrent { max });
        self
    }

    /// Force sequential processing, overriding a concurrent-capable consumer.
    pub fn sequential(mut self) -> Self {
        self.concurrency = Some(ConcurrencyModel::Sequential);
        self
    }
}
```

### 4. CamelContext::start() — concurrency resolution

Resolution order: route override > consumer declaration.

```rust
// context.rs — inside start(), after creating consumer, before spawning pipeline task

let effective = route_definition.concurrency_override()
    .unwrap_or_else(|| consumer.concurrency_model());
```

The pipeline task branches on the effective model:

```rust
match effective {
    ConcurrencyModel::Sequential => {
        // Existing loop — zero changes to current code
        while let Some(envelope) = rx.recv().await {
            let ExchangeEnvelope { exchange, reply_tx } = envelope;
            let svc = ready_with_backoff(&mut pipeline, &pipeline_cancel).await;
            let result = svc.call(exchange).await;
            handle_result(result, reply_tx);
        }
    }
    ConcurrencyModel::Concurrent { max } => {
        let sem = max.map(|n| Arc::new(Semaphore::new(n)));
        while let Some(envelope) = rx.recv().await {
            let ExchangeEnvelope { exchange, reply_tx } = envelope;
            let mut pipeline = pipeline.clone();
            let sem = sem.clone();
            let cancel = pipeline_cancel.clone();
            tokio::spawn(async move {
                let _permit = match &sem {
                    Some(s) => Some(s.acquire().await.expect("semaphore closed")),
                    None => None,
                };
                let svc = ready_with_backoff(&mut pipeline, &cancel).await;
                let result = svc.call(exchange).await;
                handle_result(result, reply_tx);
            });
        }
    }
}
```

`ready_with_backoff` extracts the existing CircuitOpen retry loop into a helper.

### 5. Backpressure model

Two levels, complementary:

| Level | Mechanism | Controls |
|-------|-----------|----------|
| Queue depth | `mpsc::channel(256)` | How many exchanges wait in line |
| Active execution | `Semaphore(N)` (optional) | How many exchanges run simultaneously |

Default for HTTP: unbounded semaphore (no semaphore). The channel buffer (256) is the only backpressure. This delegates scheduling to the tokio runtime, which is efficient for I/O-bound pipelines.

Users needing stricter control use `.concurrent(N)` to add a semaphore.

## User experience

```rust
// HTTP — automatically concurrent, zero config
RouteBuilder::from("http://0.0.0.0:8080/api")
    .process(handle_request)
    .to("log:result")
    .build()

// HTTP — user limits to 16 concurrent pipeline executions
RouteBuilder::from("http://0.0.0.0:8080/api")
    .concurrent(16)
    .process(handle_request)
    .build()

// Timer — automatically sequential, zero config
RouteBuilder::from("timer:tick?period=1s")
    .to("log:tick")
    .build()

// Force sequential on HTTP (e.g., route mutates shared state)
RouteBuilder::from("http://0.0.0.0:8080/admin")
    .sequential()
    .process(admin_handler)
    .build()
```

## Edge cases

| Scenario | Behavior |
|----------|----------|
| Pipeline failure in concurrent mode | `reply_tx` receives `Err`, spawn task exits, semaphore permit dropped |
| CircuitOpen during concurrent | Each spawned task runs its own `ready_with_backoff` loop; circuit state is `Arc<Mutex>` shared |
| Shutdown during concurrent | Cancel token stops the `while let` loop; in-flight spawned tasks finish naturally; reply channels drop |
| ErrorHandler retry in concurrent | Each spawn clones the full pipeline (includes error handler); retries are independent per-exchange |
| `Stopped` in concurrent | Spawned task receives `Err(Stopped)`, silenced same as sequential loop |
| `.concurrent()` on timer | Works but redundant — timer produces 1 exchange per tick |
| `.sequential()` on HTTP | Works — forces sequential processing; useful for stateful routes |

## Scope

### In scope
- `ConcurrencyModel` enum in `camel-component`
- `concurrency_model()` default method on `Consumer` trait
- `HttpConsumer` override returning `Concurrent { max: None }`
- `concurrency: Option<ConcurrencyModel>` field on `RouteDefinition`
- `.concurrent(N)` and `.sequential()` on `RouteBuilder`
- Branched pipeline loop in `CamelContext::start()`
- Extract `ready_with_backoff` and `handle_result` helpers from existing inline code
- Tests: concurrent HTTP pipeline, sequential override, semaphore limiting

### Out of scope
- `seda:` component (future work, uses same mechanism)
- `threads()` DSL mid-route (future work)
- Per-route channel buffer size configuration
- Metrics/observability for concurrent pipeline utilization

## Estimated effort

~3 tasks:
1. `ConcurrencyModel` + `Consumer` trait extension + `HttpConsumer` override
2. `RouteDefinition` field + `RouteBuilder` DSL (`.concurrent()`, `.sequential()`)
3. `CamelContext::start()` branched loop + helper extraction + tests
