# Pipeline Concurrency Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable concurrent pipeline execution for inbound-connection consumers (HTTP), with smart defaults and user override via the RouteBuilder DSL.

**Architecture:** Consumer declares its concurrency model via a new `concurrency_model()` default method on the `Consumer` trait. `RouteBuilder` exposes `.concurrent(N)` and `.sequential()` for user overrides. `CamelContext::start()` resolves effective concurrency (route override > consumer default) and branches between the existing sequential loop and a new spawn-per-exchange loop with optional semaphore.

**Tech Stack:** Rust, Tower, tokio (spawn, Semaphore, mpsc, oneshot), async_trait

---

### Task 1: Add `ConcurrencyModel` enum and extend `Consumer` trait

**Files:**
- Modify: `crates/camel-component/src/consumer.rs:1-80`
- Modify: `crates/camel-component/src/lib.rs:1-8`

**Step 1: Write the failing test**

Add to the bottom of the existing `#[cfg(test)] mod tests` block in `crates/camel-component/src/consumer.rs`:

```rust
#[test]
fn test_concurrency_model_default_is_sequential() {
    use super::ConcurrencyModel;

    struct DummyConsumer;

    #[async_trait::async_trait]
    impl super::Consumer for DummyConsumer {
        async fn start(&mut self, _ctx: super::ConsumerContext) -> Result<(), CamelError> {
            Ok(())
        }
        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }
    }

    let consumer = DummyConsumer;
    assert_eq!(consumer.concurrency_model(), ConcurrencyModel::Sequential);
}

#[test]
fn test_concurrency_model_concurrent_override() {
    use super::ConcurrencyModel;

    struct ConcurrentConsumer;

    #[async_trait::async_trait]
    impl super::Consumer for ConcurrentConsumer {
        async fn start(&mut self, _ctx: super::ConsumerContext) -> Result<(), CamelError> {
            Ok(())
        }
        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }
        fn concurrency_model(&self) -> ConcurrencyModel {
            ConcurrencyModel::Concurrent { max: Some(16) }
        }
    }

    let consumer = ConcurrentConsumer;
    assert_eq!(
        consumer.concurrency_model(),
        ConcurrencyModel::Concurrent { max: Some(16) }
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p camel-component -- test_concurrency_model`
Expected: FAIL — `ConcurrencyModel` not found, `concurrency_model` not a method on `Consumer`

**Step 3: Implement `ConcurrencyModel` and extend `Consumer` trait**

In `crates/camel-component/src/consumer.rs`, add the enum before the `Consumer` trait and add the default method:

```rust
/// How a consumer's exchanges should be processed by the pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConcurrencyModel {
    /// Exchanges are processed one at a time, in order. Default for polling
    /// consumers (timer, file) and synchronous consumers (direct).
    Sequential,
    /// Exchanges are processed concurrently via `tokio::spawn`. Optional
    /// semaphore limit (`max`). `None` means unbounded (channel buffer is
    /// the only backpressure).
    Concurrent { max: Option<usize> },
}
```

Extend the `Consumer` trait with a default method:

```rust
#[async_trait]
pub trait Consumer: Send + Sync {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError>;
    async fn stop(&mut self) -> Result<(), CamelError>;

    /// Declares this consumer's natural concurrency model.
    ///
    /// The runtime uses this to decide whether to process exchanges
    /// sequentially or spawn per-exchange. Consumers that accept inbound
    /// connections (HTTP, WebSocket, Kafka) should override this to return
    /// `ConcurrencyModel::Concurrent`.
    ///
    /// Default: `Sequential`.
    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }
}
```

Re-export `ConcurrencyModel` from `crates/camel-component/src/lib.rs`:

```rust
pub use consumer::{ConcurrencyModel, Consumer, ConsumerContext, ExchangeEnvelope};
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p camel-component -- test_concurrency_model`
Expected: PASS (both tests)

**Step 5: Run full workspace tests to verify no regressions**

Run: `cargo test --workspace`
Expected: All tests pass. Existing consumers don't implement `concurrency_model()` so they get the default `Sequential`.

**Step 6: Commit**

```
git add -A && git commit -m "feat: add ConcurrencyModel enum and Consumer::concurrency_model() default method"
```

---

### Task 2: HttpConsumer declares `Concurrent` concurrency model

**Files:**
- Modify: `crates/components/camel-http/src/lib.rs` (HttpConsumer `impl Consumer` block, ~line 384)

**Step 1: Write the failing test**

Add to the HTTP component's test module in `crates/components/camel-http/src/lib.rs`:

```rust
#[test]
fn test_http_consumer_declares_concurrent() {
    use camel_component::ConcurrencyModel;

    let config = HttpServerConfig {
        host: "127.0.0.1".to_string(),
        port: 19999,
        path: "/test".to_string(),
    };
    let consumer = HttpConsumer::new(config);
    assert_eq!(
        consumer.concurrency_model(),
        ConcurrencyModel::Concurrent { max: None }
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p camel-http -- test_http_consumer_declares_concurrent`
Expected: FAIL — `concurrency_model()` returns `Sequential` (default), assertion fails because we expect `Concurrent { max: None }`

**Step 3: Add `concurrency_model()` override to HttpConsumer**

In the `impl Consumer for HttpConsumer` block in `crates/components/camel-http/src/lib.rs`, add:

```rust
fn concurrency_model(&self) -> camel_component::ConcurrencyModel {
    camel_component::ConcurrencyModel::Concurrent { max: None }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p camel-http -- test_http_consumer_declares_concurrent`
Expected: PASS

**Step 5: Commit**

```
git add -A && git commit -m "feat: HttpConsumer declares Concurrent concurrency model"
```

---

### Task 3: Add concurrency field to `RouteDefinition` and DSL methods to `RouteBuilder`

**Files:**
- Modify: `crates/camel-core/src/route.rs:92-138` (RouteDefinition struct)
- Modify: `crates/camel-builder/src/lib.rs:118-217` (RouteBuilder struct + build())

**Step 1: Write the failing tests**

Add to the test module in `crates/camel-builder/src/lib.rs`:

```rust
#[test]
fn test_builder_concurrent_sets_concurrency() {
    use camel_component::ConcurrencyModel;

    let definition = RouteBuilder::from("http://0.0.0.0:8080/test")
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
        .to("log:info")
        .build()
        .unwrap();

    assert_eq!(definition.concurrency_override(), None);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p camel-builder -- test_builder_concurrent`
Run: `cargo test -p camel-builder -- test_builder_sequential`
Run: `cargo test -p camel-builder -- test_builder_default_concurrency`
Expected: FAIL — `concurrent()`, `sequential()`, `concurrency_override()` don't exist

**Step 3: Add `concurrency` field to `RouteDefinition`**

In `crates/camel-core/src/route.rs`, add the field and accessor:

```rust
use camel_component::ConcurrencyModel;

pub struct RouteDefinition {
    pub(crate) from_uri: String,
    pub(crate) steps: Vec<BuilderStep>,
    pub(crate) error_handler: Option<ErrorHandlerConfig>,
    pub(crate) circuit_breaker: Option<CircuitBreakerConfig>,
    /// User override for the consumer's concurrency model. `None` means
    /// "use whatever the consumer declares".
    pub(crate) concurrency: Option<ConcurrencyModel>,
}
```

Update `RouteDefinition::new()` to initialize `concurrency: None`.

Add accessor:

```rust
/// User-specified concurrency override, if any.
pub fn concurrency_override(&self) -> Option<&ConcurrencyModel> {
    self.concurrency.as_ref()
}
```

Add builder method:

```rust
/// Override the consumer's concurrency model for this route.
pub fn with_concurrency(mut self, model: ConcurrencyModel) -> Self {
    self.concurrency = Some(model);
    self
}
```

**Step 4: Add `concurrent()` and `sequential()` to `RouteBuilder`**

In `crates/camel-builder/src/lib.rs`, add a `concurrency` field to `RouteBuilder`:

```rust
use camel_component::ConcurrencyModel;

pub struct RouteBuilder {
    from_uri: String,
    steps: Vec<BuilderStep>,
    error_handler: Option<ErrorHandlerConfig>,
    circuit_breaker_config: Option<CircuitBreakerConfig>,
    concurrency: Option<ConcurrencyModel>,
}
```

Update `RouteBuilder::from()` to initialize `concurrency: None`.

Add methods:

```rust
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
```

Update `build()` to pass concurrency to RouteDefinition:

```rust
pub fn build(self) -> Result<RouteDefinition, CamelError> {
    if self.from_uri.is_empty() {
        return Err(CamelError::RouteError(
            "route must have a 'from' URI".to_string(),
        ));
    }
    let mut definition = RouteDefinition::new(self.from_uri, self.steps);
    if let Some(eh) = self.error_handler {
        definition = definition.with_error_handler(eh);
    }
    if let Some(cb) = self.circuit_breaker_config {
        definition = definition.with_circuit_breaker(cb);
    }
    if let Some(concurrency) = self.concurrency {
        definition = definition.with_concurrency(concurrency);
    }
    Ok(definition)
}
```

**Step 5: Run tests to verify they pass**

Run: `cargo test -p camel-builder -- test_builder_concurrent`
Run: `cargo test -p camel-builder -- test_builder_sequential`
Run: `cargo test -p camel-builder -- test_builder_default_concurrency`
Expected: All PASS

**Step 6: Run full workspace tests**

Run: `cargo test --workspace`
Expected: All pass — no API changes to existing code

**Step 7: Commit**

```
git add -A && git commit -m "feat: add concurrency field to RouteDefinition and .concurrent()/.sequential() DSL methods"
```

---

### Task 4: Implement concurrent pipeline loop in `CamelContext::start()`

This is the core change. The `start()` method in `crates/camel-core/src/context.rs:199-294` needs to:
1. Query the consumer's `concurrency_model()`
2. Resolve effective concurrency (route override > consumer default)
3. Branch between sequential loop (existing) and concurrent loop (new)

**Files:**
- Modify: `crates/camel-core/src/context.rs:199-294`

**Step 1: Write the failing integration test**

Add to `crates/camel-test/tests/integration_test.rs`:

```rust
// ---------------------------------------------------------------------------
// Test: HTTP concurrent pipeline processes multiple requests simultaneously
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_concurrent_pipeline() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(HttpComponent::new());
    ctx.register_component(mock.clone());

    // Route with a slow processor (100ms sleep). Without concurrency,
    // 5 requests would take ~500ms sequentially. With concurrency,
    // they should complete in ~100ms (all run in parallel).
    let route = RouteBuilder::from("http://0.0.0.0:18080/concurrent-test")
        .process(|ex| async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(ex)
        })
        .to("mock:concurrent-result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    // Give server time to start
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Fire 5 requests concurrently
    let client = reqwest::Client::new();
    let mut handles = Vec::new();
    let start = std::time::Instant::now();
    for i in 0..5 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            client
                .get(&format!("http://127.0.0.1:18080/concurrent-test?i={i}"))
                .send()
                .await
                .unwrap()
        }));
    }

    for handle in handles {
        let resp = handle.await.unwrap();
        assert_eq!(resp.status(), 200);
    }
    let elapsed = start.elapsed();

    ctx.stop().await.unwrap();

    // With concurrent pipeline: ~100ms (all 5 run in parallel).
    // With sequential pipeline: ~500ms (one at a time).
    // Use 350ms as threshold — generous margin but catches sequential.
    assert!(
        elapsed < std::time::Duration::from_millis(350),
        "Expected concurrent execution (<350ms), but took {:?}. \
         Pipeline may be running sequentially.",
        elapsed
    );

    let endpoint = mock.get_endpoint("concurrent-result").unwrap();
    endpoint.assert_exchange_count(5).await;
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p camel-test --test integration_test -- test_http_concurrent_pipeline`
Expected: FAIL — elapsed time >500ms because the pipeline loop is sequential

**Step 3: Implement the concurrent pipeline loop**

In `crates/camel-core/src/context.rs`, modify the `start()` method. The key changes:

1. After `endpoint.create_consumer()`, call `consumer.concurrency_model()` before moving `consumer` into the spawn.
2. Resolve effective concurrency from `RouteDefinition` override (need to thread this through — `add_route_definition` currently discards `concurrency`; preserve it on `Route`).
3. Branch the pipeline task.

First, add `concurrency` to `Route`:

In `crates/camel-core/src/route.rs`, add to `Route`:

```rust
use camel_component::ConcurrencyModel;

pub struct Route {
    pub(crate) from_uri: String,
    pub(crate) pipeline: BoxProcessor,
    pub(crate) concurrency: Option<ConcurrencyModel>,
}
```

Update `Route::new()`:

```rust
pub fn new(from_uri: impl Into<String>, pipeline: BoxProcessor) -> Self {
    Self {
        from_uri: from_uri.into(),
        pipeline,
        concurrency: None,
    }
}

pub fn with_concurrency(mut self, model: ConcurrencyModel) -> Self {
    self.concurrency = Some(model);
    self
}

pub fn concurrency_override(&self) -> Option<&ConcurrencyModel> {
    self.concurrency.as_ref()
}
```

Update `add_route_definition()` in `context.rs` to preserve concurrency:

```rust
let mut route = Route::new(definition.from_uri, pipeline);
if let Some(concurrency) = definition.concurrency {
    route = route.with_concurrency(concurrency);
}
self.add_route(route);
```

Now modify the `start()` method pipeline spawn:

```rust
// Query the consumer's concurrency model BEFORE moving it into the spawn
let consumer_concurrency = consumer.concurrency_model();

// Resolve effective concurrency: route override > consumer default
let effective_concurrency = route
    .concurrency_override()
    .cloned()
    .unwrap_or(consumer_concurrency);
```

Then branch:

```rust
use std::sync::Arc;
use tokio::sync::Semaphore;

let pipeline_cancel = self.cancel_token.child_token();
let pipeline_handle = match effective_concurrency {
    ConcurrencyModel::Sequential => {
        tokio::spawn(async move {
            // --- EXISTING sequential loop (unchanged) ---
            while let Some(envelope) = rx.recv().await {
                let ExchangeEnvelope { exchange, reply_tx } = envelope;
                let svc = loop {
                    match pipeline.ready().await {
                        Ok(svc) => break svc,
                        Err(CamelError::CircuitOpen(ref msg)) => {
                            tracing::warn!("Circuit open, backing off: {msg}");
                            tokio::select! {
                                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => continue,
                                _ = pipeline_cancel.cancelled() => {
                                    if let Some(tx) = reply_tx {
                                        let _ = tx.send(Err(CamelError::CircuitOpen(msg.clone())));
                                    }
                                    return;
                                }
                            }
                        }
                        Err(e) => {
                            error!("Pipeline not ready: {e}");
                            if let Some(tx) = reply_tx { let _ = tx.send(Err(e)); }
                            return;
                        }
                    }
                };
                let result = svc.call(exchange).await;
                if let Some(tx) = reply_tx {
                    let _ = tx.send(result);
                } else if let Err(ref e) = result {
                    if !matches!(e, CamelError::Stopped) {
                        error!("Pipeline error: {e}");
                    }
                }
            }
        })
    }
    ConcurrencyModel::Concurrent { max } => {
        let sem = max.map(|n| Arc::new(Semaphore::new(n)));
        tokio::spawn(async move {
            while let Some(envelope) = rx.recv().await {
                let ExchangeEnvelope { exchange, reply_tx } = envelope;
                let mut pipe_clone = pipeline.clone();
                let sem = sem.clone();
                let cancel = pipeline_cancel.clone();
                tokio::spawn(async move {
                    // Acquire semaphore permit if bounded
                    let _permit = match &sem {
                        Some(s) => Some(
                            s.acquire().await.expect("semaphore closed")
                        ),
                        None => None,
                    };
                    // Ready loop with circuit breaker backoff
                    let svc = loop {
                        match pipe_clone.ready().await {
                            Ok(svc) => break svc,
                            Err(CamelError::CircuitOpen(ref msg)) => {
                                tracing::warn!("Circuit open, backing off: {msg}");
                                tokio::select! {
                                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => continue,
                                    _ = cancel.cancelled() => {
                                        if let Some(tx) = reply_tx {
                                            let _ = tx.send(Err(CamelError::CircuitOpen(msg.clone())));
                                        }
                                        return;
                                    }
                                }
                            }
                            Err(e) => {
                                error!("Pipeline not ready: {e}");
                                if let Some(tx) = reply_tx { let _ = tx.send(Err(e)); }
                                return;
                            }
                        }
                    };
                    let result = svc.call(exchange).await;
                    if let Some(tx) = reply_tx {
                        let _ = tx.send(result);
                    } else if let Err(ref e) = result {
                        if !matches!(e, CamelError::Stopped) {
                            error!("Pipeline error: {e}");
                        }
                    }
                });
            }
        })
    }
};
self.tasks.push(pipeline_handle);
```

**Step 4: Run the integration test**

Run: `cargo test -p camel-test --test integration_test -- test_http_concurrent_pipeline`
Expected: PASS — 5 requests complete in ~100ms, well under 350ms threshold

**Step 5: Run full workspace tests**

Run: `cargo test --workspace`
Expected: All tests pass. Sequential consumers (timer, file, direct) still use the existing loop.

**Step 6: Commit**

```
git add -A && git commit -m "feat: concurrent pipeline loop in CamelContext::start() with semaphore support"
```

---

### Task 5: Integration test — `.sequential()` override on HTTP route

Verifies that a user can force an HTTP route back to sequential processing.

**Files:**
- Modify: `crates/camel-test/tests/integration_test.rs`

**Step 1: Write the test**

```rust
// ---------------------------------------------------------------------------
// Test: HTTP route with .sequential() override processes one at a time
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_sequential_override() {
    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(HttpComponent::new());
    ctx.register_component(mock.clone());

    // Same slow processor, but forced sequential via .sequential()
    let route = RouteBuilder::from("http://0.0.0.0:18081/sequential-test")
        .sequential()
        .process(|ex| async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(ex)
        })
        .to("mock:sequential-result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Fire 3 requests concurrently
    let client = reqwest::Client::new();
    let mut handles = Vec::new();
    let start = std::time::Instant::now();
    for i in 0..3 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            client
                .get(&format!("http://127.0.0.1:18081/sequential-test?i={i}"))
                .send()
                .await
                .unwrap()
        }));
    }

    for handle in handles {
        let resp = handle.await.unwrap();
        assert_eq!(resp.status(), 200);
    }
    let elapsed = start.elapsed();

    ctx.stop().await.unwrap();

    // Sequential: ~300ms (3 * 100ms). Must be clearly above concurrent threshold.
    assert!(
        elapsed >= std::time::Duration::from_millis(250),
        "Expected sequential execution (>=250ms), but took {:?}. \
         Pipeline may be running concurrently despite .sequential() override.",
        elapsed
    );

    let endpoint = mock.get_endpoint("sequential-result").unwrap();
    endpoint.assert_exchange_count(3).await;
}
```

**Step 2: Run test**

Run: `cargo test -p camel-test --test integration_test -- test_http_sequential_override`
Expected: PASS — elapsed is ~300ms, confirming sequential execution

**Step 3: Commit**

```
git add -A && git commit -m "test: verify .sequential() override forces HTTP route to sequential processing"
```

---

### Task 6: Integration test — `.concurrent(N)` semaphore limiting

Verifies that semaphore limiting actually constrains parallelism.

**Files:**
- Modify: `crates/camel-test/tests/integration_test.rs`

**Step 1: Write the test**

```rust
// ---------------------------------------------------------------------------
// Test: HTTP route with .concurrent(2) limits parallelism to 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_concurrent_with_semaphore_limit() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let mock = MockComponent::new();
    let mut ctx = CamelContext::new();
    ctx.register_component(HttpComponent::new());
    ctx.register_component(mock.clone());

    let peak = Arc::new(AtomicUsize::new(0));
    let current = Arc::new(AtomicUsize::new(0));
    let peak_clone = peak.clone();
    let current_clone = current.clone();

    // Limit to 2 concurrent pipeline executions
    let route = RouteBuilder::from("http://0.0.0.0:18082/semaphore-test")
        .concurrent(2)
        .process(move |ex| {
            let peak = peak_clone.clone();
            let current = current_clone.clone();
            async move {
                let val = current.fetch_add(1, Ordering::SeqCst) + 1;
                // Update peak
                peak.fetch_max(val, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                current.fetch_sub(1, Ordering::SeqCst);
                Ok(ex)
            }
        })
        .to("mock:semaphore-result")
        .build()
        .unwrap();

    ctx.add_route_definition(route).unwrap();
    ctx.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Fire 6 requests concurrently
    let client = reqwest::Client::new();
    let mut handles = Vec::new();
    for i in 0..6 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            client
                .get(&format!("http://127.0.0.1:18082/semaphore-test?i={i}"))
                .send()
                .await
                .unwrap()
        }));
    }

    for handle in handles {
        let resp = handle.await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    ctx.stop().await.unwrap();

    // Peak concurrency should be exactly 2 (semaphore limit)
    let peak_val = peak.load(Ordering::SeqCst);
    assert!(
        peak_val <= 2,
        "Expected peak concurrency <= 2, but got {}. Semaphore not working.",
        peak_val
    );

    let endpoint = mock.get_endpoint("semaphore-result").unwrap();
    endpoint.assert_exchange_count(6).await;
}
```

**Step 2: Run test**

Run: `cargo test -p camel-test --test integration_test -- test_http_concurrent_with_semaphore_limit`
Expected: PASS — peak concurrency is 2

**Step 3: Run full workspace tests one final time**

Run: `cargo test --workspace`
Expected: All tests pass

**Step 4: Commit**

```
git add -A && git commit -m "test: verify .concurrent(N) semaphore limits pipeline parallelism"
```

---

### Task 7: Verify all existing tests still pass, update analysis doc

**Files:**
- Modify: `docs/plans/2026-03-01-pipeline-concurrency-analysis.md` (update "Decision Pending" section)

**Step 1: Run full test suite**

Run: `cargo test --workspace`
Expected: All tests pass (existing + 3 new integration tests + 2 new unit tests)

**Step 2: Update analysis doc**

Replace the "Decision Pending" section at the bottom of `docs/plans/2026-03-01-pipeline-concurrency-analysis.md` with:

```markdown
## Decision: F+C Combined

Chosen: **Option F + Option C combined** — consumer declares its concurrency model via `Consumer::concurrency_model()`, user can override per-route with `.concurrent(N)` or `.sequential()` on `RouteBuilder`.

See `2026-03-01-pipeline-concurrency-design.md` for the full design.
```

**Step 3: Commit**

```
git add -A && git commit -m "docs: mark pipeline concurrency analysis as decided (F+C combined)"
```
