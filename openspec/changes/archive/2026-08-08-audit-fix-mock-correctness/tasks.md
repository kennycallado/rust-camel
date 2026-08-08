# Tasks: audit-fix-mock-correctness

## camel-component-mock

### Task M1: Wire fail_fast_error dead feature — trigger + assert_satisfied

**Files:**
- `crates/components/camel-mock/src/lib.rs` (modified)
- `crates/components/camel-mock/Cargo.toml` (modified — add `futures` dev-dependency)

**Steps:**
1. Add `futures.workspace = true` to `[dev-dependencies]` in `Cargo.toml` (needed for `FutureExt::catch_unwind` in async panic-catch tests; also needed by M2 stream tests).
2. Add a `pub fn trigger_fail_fast(&self, error: CamelError)` method on `MockEndpointInner` (after the existing `fail_fast_error` getter, around line 543). Use the same `if let Ok(mut guard)` pattern as `reset()` (line 323) to avoid panics on poison: `if let Ok(mut guard) = self.fail_fast_error.lock() { *guard = Some(error); }`. Note: `fail_fast_error` is a `std::sync::Mutex` (not tokio), so the lock is synchronous.
3. In `assert_satisfied()` (starts at line 452), add a private helper `fn set_fail_fast_on_mismatch(&self)` that locks `self.fail_fast_error` (same `if let Ok(mut guard)` pattern) and sets `*guard = Some(CamelError::ProcessorError("assert_satisfied expectation mismatch".to_string()))` when `self.fail_fast` is true. Insert a call to `self.set_fail_fast_on_mismatch()` BEFORE each expectation-mismatch `panic!`. There are 5 expectation-mismatch panic sites: body count mismatch (line 464), any-order body not found (line 482), in-order body mismatch (line 491), expected header missing (line 505), header regex missing (line 530). Line 516 (invalid regex compile) is a programming error, NOT an expectation mismatch — intentionally excluded. Note: `set_fail_fast_on_mismatch` locks `fail_fast_error` which is a distinct mutex from `expectations` (locked via `self.expectations.lock().expect(...)` at line 459) — the guard is dropped before any panic, so no deadlock.
4. In `body_eq` (line 547), no change needed — it already returns `false` for `Body::Stream` which is correct.

**Tests:** (executable spec — name, setup, action, assert)

Canonical test setup flow (from existing tests around lib.rs:1730-1737): `component.create_endpoint("mock:<name>", &NoOpComponentContext) -> Box<dyn Endpoint>`, then `component.get_endpoint("<name>").unwrap() -> Arc<MockEndpointInner>` for the inner, then `endpoint.create_producer(rt(), &test_producer_ctx()) -> BoxProcessor` for the producer (note: `rt()` returns an owned `Arc<dyn RuntimeObservability>`, passed by value — no `&`). The `MockEndpoint(Arc<MockEndpointInner>)` wrapper is created by `create_endpoint` and stored in the component registry; `get_endpoint` returns the `Arc<MockEndpointInner>` directly.

- `test_trigger_fail_fast_rejects_subsequent_producer`: Create `MockComponent::with_config(MockConfig { fail_fast: true, ..Default::default() })`. Call `component.create_endpoint("mock:test", &NoOpComponentContext)` to get the endpoint. Call `component.get_endpoint("test").unwrap()` to get `Arc<MockEndpointInner>`. Call `inner.trigger_fail_fast(CamelError::ProcessorError("boom".to_string()))`. Create producer via `endpoint.create_producer(rt(), &test_producer_ctx())`. Call `producer.ready()` then `producer.call(Exchange::default()).await`. Assert `Err(CamelError::ProcessorError(msg))` where `msg.contains("fail-fast mode")` and `!msg.contains("boom")`.
- `test_trigger_fail_fast_noop_when_fail_fast_false`: Same flow but `fail_fast: false`. Call `trigger_fail_fast(CamelError::ProcessorError("boom".to_string()))`. Assert producer returns `Ok(exchange)`.
- `test_reset_clears_trigger_fail_fast`: Create endpoint with `fail_fast: true`, call `trigger_fail_fast(CamelError::ProcessorError("boom".to_string()))`, call `inner.reset().await`, assert `inner.fail_fast_error()` returns `None` and producer returns `Ok`.
- `test_assert_satisfied_body_count_mismatch_sets_fail_fast`: Create endpoint with `fail_fast: true`. Set `inner.expect_body(Body::Text("a".to_string()))` and `inner.expect_body(Body::Text("b".to_string()))`. Send 1 exchange via producer. Call `use futures::FutureExt; AssertUnwindSafe(inner.assert_satisfied()).catch_unwind().await`. Assert result is `Err(_)` (panic caught). Assert `inner.fail_fast_error()` returns `Some`.
- `test_assert_satisfied_body_mismatch_sets_fail_fast`: Create endpoint with `fail_fast: true`, set expectation `Body::Text("expected".to_string())`. Send exchange with `Body::Text("actual".to_string())`. Call `AssertUnwindSafe(inner.assert_satisfied()).catch_unwind().await`. Assert result is `Err(_)` AND `inner.fail_fast_error()` is `Some`.
- `test_assert_satisfied_no_set_error_when_fail_fast_false`: Create endpoint with `fail_fast: false`, set unmet expectation. Call `AssertUnwindSafe(inner.assert_satisfied()).catch_unwind().await`. Assert result is `Err(_)` AND `inner.fail_fast_error()` is `None`.

**Acceptance:**
- `cargo test -p camel-component-mock --lib` — all existing + new tests pass
- `cargo clippy -p camel-component-mock --all-targets -- -D warnings` — clean
- `trigger_fail_fast` method exists and is `pub` on `MockEndpointInner`
- All 5 expectation-mismatch panic sites in `assert_satisfied` set `fail_fast_error` before panicking when `self.fail_fast` is true

- [x] M1

### Task M2: Fix clone_body to preserve Body::Stream

**Files:**
- `crates/components/camel-mock/src/lib.rs` (modified)

**Steps:**
1. In `clone_body` (line 686), add an explicit arm for `Body::Stream` BEFORE the wildcard: `camel_component_api::Body::Stream(s) => camel_component_api::Body::Stream(s.clone()),`. This mirrors the `Body::Clone` impl in `crates/camel-api/src/body.rs:187`.
2. Replace the comment on the wildcard arm (line 693) from "Streams and future uncloneable variants fall back to Empty." to "Safety net for future #[non_exhaustive] variants; all current variants are handled explicitly above.".
3. Verify the wildcard arm is unreachable for all current `Body` variants (Empty, Bytes, Text, Json, Xml, Stream) — all 6 are now handled explicitly.

**Tests:** (executable spec — name, setup, action, assert)
- `test_clone_body_preserves_stream`: Use `use camel_component_api::{StreamBody, StreamMetadata}; use futures::stream; use bytes::Bytes; use std::sync::Arc; use tokio::sync::Mutex;`. Create a `Body::Stream(StreamBody { stream: Arc::new(Mutex::new(Some(Box::pin(stream::iter(vec![Ok(Bytes::from("data"))]))))), metadata: StreamMetadata::default() })`. Create `MockComponent::with_config(MockConfig { copy_on_exchange: true, ..Default::default() })`. Call `component.create_endpoint("mock:test", &NoOpComponentContext)` → endpoint. Call `component.get_endpoint("test").unwrap()` → inner. Create producer. Send exchange with the stream body. Retrieve recorded exchange via `inner.get_received_exchanges().await`. Assert `matches!(recorded[0].input.body, Body::Stream(_))` (NOT `Body::Empty`).
- `test_clone_body_stream_shares_arc`: This is a direct unit test of `clone_body` (no producer/mock setup needed). Use `use camel_component_api::{Body, StreamBody, StreamMetadata}; use camel_api::error::CamelError; use futures::stream; use bytes::Bytes; use std::sync::Arc; use tokio::sync::Mutex;`. Create `original = Body::Stream(StreamBody { stream: Arc::new(Mutex::new(Some(Box::pin(stream::iter(vec![Ok(Bytes::from("data"))]))))), metadata: StreamMetadata::default() })`. Call `let clone = clone_body(&original)`. Consume the original: `original.into_bytes(100).await.unwrap()`. Then consume the clone: `let result = clone.into_bytes(100).await`. Assert `matches!(result, Err(CamelError::AlreadyConsumed))` — proving `Arc::clone` semantics (shared handle), not deep copy.

**Acceptance:**
- `cargo test -p camel-component-mock --lib` — all existing + new tests pass
- `cargo clippy -p camel-component-mock --all-targets -- -D warnings` — clean
- `clone_body` has explicit `Body::Stream(s) => Body::Stream(s.clone())` arm
- No comment claiming streams are "uncloneable"

- [x] M2
