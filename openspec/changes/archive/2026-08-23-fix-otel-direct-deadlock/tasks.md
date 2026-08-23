# Tasks: fix-otel-direct-deadlock

## Phase 1: Fix TracingProcessor tower-contract violation (rc-qoq3)

### camel-core

#### Task 1.1: TracingProcessor consumes the readied original inner

**Files:**
- `crates/camel-core/src/shared/observability/adapters/tracer.rs` (modified)
- `crates/services/camel-otel/tests/integration.rs` (modified — new span-status test)

**Steps:**
1. In `TracingProcessor::call`, replace the clone-then-ready pattern. Current code: `let mut inner = self.inner.clone();` (≈line 177) and later `inner.ready().await?.call(exchange).await` (≈line 191). New code: `let fresh = self.inner.clone(); let mut inner = std::mem::replace(&mut self.inner, fresh);` — clone first, then replace (avoids E0502). The fresh clone stays in `self.inner` as the unreadied placeholder for the next cycle.
2. Replace `inner.ready().await?.call(exchange).await` with `inner.call(exchange).await` — do NOT re-ready: `TracingProcessor::poll_ready` already readied the ORIGINAL inner, and its reservations (e.g. `DirectProducer::pending_permit`) belong to that instance.
3. Leave `poll_ready` delegation (`self.inner.poll_ready(cx)`) and the `Clone` impl untouched. Leave span lifecycle code (span_builder, `SpanEndGuard`, status recording) untouched.
4. Verify no other use of `self.inner` inside `call` after the replace (the future must own `inner` exclusively).

**Tests:** (executable spec)
- `tracing_processor_does_not_re_ready_clone`: mock inner `Service<Exchange>` in the existing `tracer.rs` test module whose `poll_ready` acquires the sole permit of a shared `tokio::sync::Semaphore::new(1)` into a `pending_permit: Option<OwnedSemaphorePermit>` field, whose `Clone` does `semaphore: Arc::clone(...), pending_permit: None`, and whose `call` returns `Err(ProcessorError)` if `pending_permit` is None else `Ok(exchange)` after `pending_permit.take()` → build `TracingProcessor::new(mock_inner, "r".into(), 0, DetailLevel::Minimal, None)` → `tokio::time::timeout(Duration::from_secs(5), tracing_proc.ready_and_call_one(exchange))` style drive (use `tower::ServiceExt::ready().await.unwrap().call(ex).await` on the TracingProcessor) → assert completes within 5s and returns `Ok`. Command: `cargo test -p camel-core --lib tracing_processor_does_not_re_ready_clone`. Expected: fails (hangs → timeout panic) before the fix, passes after.
- `tracing_processor_reusable_across_sequential_cycles`: same mock; drive `ready().await?.call(ex_a).await` then on the SAME TracingProcessor instance `ready().await?.call(ex_b).await` → both cycles complete within 5s each, second returns Ok. Command: `cargo test -p camel-core --lib tracing_processor_reusable_across_sequential_cycles`. Expected: fails before fix, passes after.
- `span_status_success_and_error_exported`: NEW test in `crates/services/camel-otel/tests/integration.rs` (modified file; it already wires TracingProcessor with an in-memory span exporter) — wrap an Ok-returning inner, drive one exchange → assert exactly one exported span with `Status::Ok`; wrap an Err-returning inner (`Err(ProcessorError(...))`), drive one exchange → assert its span has error status. Command: `cargo test -p camel-otel --test integration span_status_success_and_error_exported`. Expected: passes before and after Task 1.1's code change (regression guard for span lifecycle).

**Acceptance:**
- `cargo test -p camel-core --lib` passes including the two new tests.
- `cargo test -p camel-otel --test integration` passes including `span_status_success_and_error_exported`.
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo clippy -p camel-otel -- -D warnings` exits 0.
- `cargo fmt --check` clean for tracer.rs and integration.rs.

- [x] 1.1

### camel-test

#### Task 1.2: E2E regression — direct hop with tracing enabled completes and repeats

**Files:**
- `crates/camel-test/tests/otel_direct_hop_regression.rs` (new)

**Steps:**
1. New integration test modeled on the wiring in `crates/camel-test/tests/direct_top_level_test.rs` (read it first for the CamelContext + route-registration pattern) and `crates/camel-test/tests/tracer_test.rs` (read it for how a traced pipeline / TracerConfig is enabled in tests — no OTLP endpoint is needed; the defect is exporter-independent, a noop/global provider suffices).
2. Build a context with tracing enabled at the route-compiler level (same mechanism `tracer_test.rs` uses — `TracerConfig`/`with_tracing` path on the context or controller; reuse exactly what that test does).
3. Register two routes: `entry` with a step `to: "direct:echo"`, and `echo` consuming from `direct:echo` with one `set_header` step (mirror the builder DSL used in `direct_top_level_test.rs` for `to(...)` and `from("direct:...")`).
4. Start the context, wait for direct consumer registration (the `direct_top_level_test.rs` pattern — poll/await startup completion).
5. Drive TWO sequential InOut exchanges through the entry pipeline with `tokio::time::timeout(Duration::from_secs(5), ...)` around each await.

**Tests:** (executable spec)
- `otel_enabled_direct_hop_completes_and_repeats`: context with tracing enabled + entry(`to: direct:echo`) + echo routes started → send InOut exchange A through entry pipeline within 5s timeout → assert Ok and echo's header effect present → send exchange B the same way → assert Ok within 5s (the permit-wedge failure would hang the second). Command: `cargo test -p camel-test --test otel_direct_hop_regression`. Expected: fails (first or second exchange times out) before Task 1.1's fix, passes after.

**Acceptance:**
- `cargo test -p camel-test --test otel_direct_hop_regression` passes.
- Test file contains no `#[ignore]`.
- `cargo clippy -p camel-test -- -D warnings` exits 0 (or the workspace's established clippy invocation for camel-test).

- [x] 1.2

## Phase 2: Readiness hardening for seven producers (rc-y9l3)

Note: all Phase 2 tests live in each producer's in-file `#[cfg(test)] mod tests` (child module), so private fields (`semaphore`/`sem`, `stopped`, `init_failed`) are directly accessible — "test-visible Arc" below means `Arc::clone(&producer.semaphore)` from the test module; no accessor needed.

### camel-direct

#### Task 2.1: DirectProducer — permit acquisition moves into call

**Files:**
- `crates/components/camel-direct/src/lib.rs` (modified)

**Steps:**
1. Delete fields `pending_permit` and `acquire_fut` from `DirectProducer` and from its `Clone` impl; keep `semaphore: Arc<Semaphore>`.
2. Rewrite `poll_ready`: keep ONLY the registry check (None + `fail_if_no_consumers != Some(false)` → `Poll::Ready(Err(EndpointCreationFailed("direct endpoint '{name}' not registered")))`; closed sender → `Poll::Ready(Err(EndpointCreationFailed("direct endpoint '{name}' channel closed")))`); when a live consumer exists → `Poll::Ready(Ok(()))`. No semaphore interaction.
3. In `call`, drop the `pending_permit.take()` error branch. Before `Box::pin`, clone the semaphore Arc into a local (`let semaphore = Arc::clone(&self.semaphore);`). Inside the future, acquire the permit FIRST and OUTSIDE the `tokio::time::timeout` wrapper (`let _permit = semaphore.acquire_owned().await.map_err(|_| CamelError::ChannelClosed)?;`) so permit contention never eats the dispatch timeout budget; the existing timeout then wraps only the registry lookup → `sender.send(...)` → `reply_rx.await` round-trip, unchanged. (Arc clone before pinning avoids capturing `&self` in a `'static` future.)
4. Update in-file tests that construct or assert on `pending_permit`/`poll_ready` permit behavior; keep the absent-consumer fail-fast test green.
5. Verify `crates/components/camel-direct/CONTEXT.md` still documents the residual operator startup-ordering window (section around lines 37-44). The semaphore wording may change, but the residual-window documentation MUST remain — it owns the direct-startup-handshake "residual operator window" scenario.

**Tests:**
- `poll_ready_absent_consumer_fails_fast`: registry empty, `fail_if_no_consumers` unset → `poll_ready(&mut cx)` returns `Poll::Ready(Err(EndpointCreationFailed(_)))`. Command: `cargo test -p camel-component-direct --lib`. Expected: passes before and after.
- `poll_ready_closed_channel_fails_fast`: registry holds a dropped (closed) sender for the name → `poll_ready` returns `Poll::Ready(Err(EndpointCreationFailed(_)))` with "channel closed" in the message. Command: `cargo test -p camel-component-direct --lib`. Expected: passes before and after.
- `poll_ready_live_consumer_ok_without_permit`: registered live consumer → `poll_ready` returns `Poll::Ready(Ok(()))` and semaphore `available_permits() == 1` (no permit taken). Command: `cargo test -p camel-component-direct --lib`. Expected: fails before (permit consumed → available 0), passes after.
- `call_blocks_on_semaphore_until_release`: TWO-call proof (spec scenario second half): register a consumer whose pipeline receives the exchange but parks the reply on a test-controlled `oneshot`; drive call A → in-flight awaiting the reply; drive call B → poll → Pending on the semaphore while A awaits; signal the consumer reply → A completes Ok and B proceeds past acquisition and completes. Command: `cargo test -p camel-component-direct --lib`. Expected: passes after.
- `call_pending_when_all_permits_held`: external-hold proof (spec scenario first half): `try_acquire_owned()` the sole permit via `Arc::clone(&producer.semaphore)` from the in-file test module, poll a `call` future → Pending; drop the permit → future proceeds past acquisition. Command: same. Expected: passes after.

**Acceptance:**
- `cargo test -p camel-component-direct` (lib + integration) passes.
- `cargo clippy -p camel-component-direct -- -D warnings` exits 0.

- [x] 2.1

### camel-kafka

#### Task 2.2: KafkaProducer — permit acquisition moves into call

**Files:**
- `crates/components/camel-kafka/src/producer.rs` (modified)

**Steps:**
1. Delete `pending_permit`/`acquire_fut` fields and their `Clone` entries; keep `semaphore`.
2. `poll_ready`: keep ONLY the stopped check (`self.stopped.load(Ordering::SeqCst)` → `Poll::Ready(Err(ProcessorError("Kafka producer is stopped")))`); otherwise `Poll::Ready(Ok(()))`.
3. `call`: remove `let permit = self.pending_permit.take();` and the `permit.ok_or_else(...)` branch. Before `Box::pin`, clone the semaphore Arc into a local; inside the async block first `let _permit = semaphore.acquire_owned().await.map_err(|_| CamelError::ChannelClosed)?;` then existing topic/payload/delivery logic unchanged. (Arc clone before pinning — no `&self` capture.)
4. Update in-file tests referencing permit readiness.

**Tests:**
- `poll_ready_stopped_returns_error`: `stopped` flag set → `poll_ready` returns `Poll::Ready(Err(ProcessorError(_)))` with "stopped" in message. Command: `cargo test -p camel-component-kafka --lib`. Expected: passes before and after.
- `poll_ready_running_returns_ok_without_permit`: `stopped` unset → `Poll::Ready(Ok(()))` and `semaphore.available_permits()` equals the producer's limit (`Semaphore::new(config.max_poll_records)`, default 500 — in the test, read the same config value the producer was built with and assert equality; if config clamping prevents max_poll_records=1, assert against the built config's effective value). Command: same. Expected: fails before, passes after.
- `call_blocks_on_semaphore_until_release`: drain ALL permits externally (`while let Ok(p) = Arc::clone(&producer.semaphore).try_acquire_owned() { held.push(p) }`), spawn `call` future, poll → Pending while drained; drop the held permits → future proceeds past acquisition (assertion target is the Pending→progress transition, not the broker result). Command: same. Expected: passes after.

**Acceptance:**
- `cargo test -p camel-component-kafka --lib` passes.
- `cargo clippy -p camel-component-kafka --all-targets -- -D warnings` exits 0.

- [x] 2.2

### camel-jms

#### Task 2.3: JmsProducer — unconditional readiness

**Files:**
- `crates/components/camel-jms/src/producer.rs` (modified)
- `CONTEXT-MAP.md` (modified — ConsumerStopping glossary entry)

**Steps:**
1. Delete `pending_permit`/`acquire_fut` fields and their `Clone` entries; keep `semaphore`.
2. `poll_ready`: return `Poll::Ready(Ok(()))` unconditionally (delete permit machinery; the only other check today is the closed-semaphore error, which moves to `call`).
3. `call`: remove `pending_permit.take()` branch. Before `Box::pin`, clone the semaphore Arc into a local; inside the async block first `let _permit = semaphore.acquire_owned().await.map_err(|_| CamelError::ConsumerStopping)?;` (keep the existing error variant the file already uses for closed semaphore) then existing send logic unchanged. (Arc clone before pinning.)
4. Rewrite the existing test `poll_ready_returns_consumer_stopping_when_semaphore_closed` (producer.rs ≈line 311): closed semaphore no longer errors in `poll_ready` — it now returns `Ok(())`, and the error surfaces in `call`. Replace with a call-level test below; do not preserve the poll_ready error branch to keep it green.
5. Update the `ConsumerStopping` glossary entry in `CONTEXT-MAP.md` (≈line 152): it currently says "producer `poll_ready` shutdown signals" — amend to state the signal is raised in `call()` (moved from `poll_ready` by this change) while keeping the ADR-0024 origin and crate references.

**Tests:**
- `poll_ready_returns_ok_unconditionally`: fresh producer with `concurrency_limit` 1, no broker, semaphore open → `Poll::Ready(Ok(()))` and `semaphore.available_permits() == 1`. Command: `cargo test -p camel-component-jms --lib`. Expected: fails before, passes after.
- `call_closed_semaphore_returns_error`: `semaphore.close()`  then drive `call` future → returns `Err(ConsumerStopping)` (acquisition failure surfaced in call). Command: same. Expected: passes after.
- `call_blocks_on_semaphore_until_release`: producer with `concurrency_limit` 1; hold the sole permit externally (`Arc::clone(&producer.semaphore).try_acquire_owned()`), spawn `call` future, poll → Pending while held; drop permit → future proceeds past acquisition. Command: same. Expected: passes after.

**Acceptance:**
- `cargo test -p camel-component-jms --lib` passes.
- `cargo clippy -p camel-component-jms -- -D warnings` exits 0.

- [x] 2.3

### camel-cxf

#### Task 2.4: CxfProducer — semaphore out of poll_ready, bridge-state machine kept

**Files:**
- `crates/components/camel-cxf/src/producer.rs` (modified)

**Steps:**
1. Delete `pending_permit`/`acquire_fut` fields and their `Clone` entries; keep `semaphore`.
2. `poll_ready`: delete the "Phase 1: backpressure via semaphore" block entirely; KEEP the "Phase 2: bridge state check" block byte-for-byte semantics: `Ready` → `Poll::Ready(Ok(()))`; `Starting`/`Restarting` → waker-spawn + `Poll::Pending`; `Degraded(reason)` → `Poll::Ready(Err(ProcessorError("cxf bridge degraded: {reason}")))`; `Stopped` → `Poll::Ready(Err(ProcessorError("cxf bridge stopped")))`; no slot → `Poll::Ready(Ok(()))`.
3. `call`: remove `pending_permit.take()`/permit error branch. Before `Box::pin`, clone the semaphore Arc into a local; inside the async block first `let _permit = semaphore.acquire_owned().await.map_err(|_| CamelError::ProcessorError("cxf producer semaphore closed".into()))?;` then existing request logic unchanged. (Arc clone before pinning.)
4. Update in-file tests referencing permit readiness.

**Tests:**
- `poll_ready_ready_state_ok_without_permit`: bridge slot in `Ready` → `Poll::Ready(Ok(()))`, `semaphore.available_permits() == 1` (cxf uses `Semaphore::new(1)`). Command: `cargo test -p camel-component-cxf --lib`. Expected: fails before, passes after.
- `poll_ready_starting_returns_pending`: bridge slot in `Starting` → `poll_ready` returns `Poll::Pending`; the waker path with `Handle::try_current` fallback (no runtime in unit test → immediate wake) makes this unit-testable per the existing pattern in producer.rs:223-236. Command: same. Expected: passes before and after (behavior preserved).
- `poll_ready_degraded_and_stopped_error`: slot in `Degraded("x")` → Err containing "degraded"; slot in `Stopped` → Err containing "stopped". Command: same. Expected: passes before and after (behavior preserved).
- `call_blocks_on_semaphore_until_release`: hold the sole permit externally (clone `Arc::clone(&producer.semaphore)` from the in-file test module — cxf semaphore is `Semaphore::new(1)`), spawn `call` future, poll → Pending while held; drop permit → future proceeds past acquisition. Command: same. Expected: passes after.

**Acceptance:**
- `cargo test -p camel-component-cxf --lib` passes.
- `cargo clippy -p camel-component-cxf -- -D warnings` exits 0.

- [x] 2.4

### camel-component-wasm

#### Task 2.5: WasmProducer — permit acquisition moves into call

**Files:**
- `crates/components/camel-component-wasm/src/producer.rs` (modified)

**Steps:**
1. Delete `pending_permit`/`acquire_fut` fields and their `Clone` entries; keep `sem`.
2. `poll_ready`: keep ONLY the init-failed check (`self.init_failed.load(Ordering::Relaxed)` → `Poll::Ready(Err(ProcessorError("wasm runtime initialization failed")))`); otherwise `Poll::Ready(Ok(()))`.
3. `call`: remove `let permit = self.pending_permit.take();`. Before `Box::pin`, clone the semaphore Arc into a local; inside the async block, at or before the current `permit.expect(...)` site (producer.rs ≈line 237), acquire a NAMED permit: `let permit = semaphore.acquire_owned().await.map_err(|_| CamelError::ProcessorError("wasm producer semaphore closed".into()))?;` and thread it into `runtime.process_streaming_exchange(..., permit, ...)` (≈line 255) exactly where the previously-taken permit flowed. Existing module invocation logic otherwise unchanged.
4. Update in-file tests referencing permit readiness.

**Tests:**
- `poll_ready_init_failed_returns_error`: `init_failed` set → `Poll::Ready(Err(ProcessorError(_)))` containing "initialization failed". Command: `cargo test -p camel-component-wasm --lib`. Expected: passes before and after.
- `poll_ready_healthy_ok_without_permit`: `init_failed` unset, producer built with `max_concurrent_calls` 1 → `Poll::Ready(Ok(()))`, `producer.sem.available_permits() == 1`. Command: same. Expected: fails before, passes after.
- `call_blocks_on_semaphore_until_release`: producer with `max_concurrent_calls` 1; hold the sole permit externally (`Arc::clone(&producer.sem).try_acquire_owned()` — the wasm field is `sem`, not `semaphore`), spawn `call` future, poll → Pending while held; drop permit → future proceeds past acquisition. Command: same. Expected: passes after.

**Acceptance:**
- `cargo test -p camel-component-wasm --lib` passes.
- `cargo clippy -p camel-component-wasm -- -D warnings` exits 0.

- [x] 2.5

### camel-opensearch

#### Task 2.6: OpenSearchProducer — unconditional readiness

**Files:**
- `crates/components/camel-opensearch/src/producer/mod.rs` (modified)
- `CONTEXT-MAP.md` (modified — ConsumerStopping glossary entry, jms+opensearch co-owned; only touch if Task 2.3 has not already amended it)

**Steps:**
1. Delete `pending_permit`/`acquire_fut` fields and their `Clone` entries; keep `semaphore`.
2. `poll_ready`: return `Poll::Ready(Ok(()))` unconditionally (the only other check today is the closed-semaphore error, which moves to `call`).
3. `call`: remove permit-take branch. Before `Box::pin`, clone the semaphore Arc into a local; inside the async block first `let _permit = semaphore.acquire_owned().await.map_err(|_| CamelError::ConsumerStopping)?;` (keep the existing error variant the file already uses for closed semaphore, producer/mod.rs:595) then existing client/request logic unchanged (client lazily created as before). (Arc clone before pinning.)
4. Rewrite the existing test `poll_ready_returns_consumer_stopping_when_semaphore_closed` (producer/mod.rs ≈line 716): closed semaphore no longer errors in `poll_ready` — it returns `Ok(())` and the error surfaces in `call`. Replace with `call_closed_semaphore_returns_error` below; do not preserve the poll_ready error branch.

**Tests:**
- `poll_ready_returns_ok_unconditionally`: fresh producer (no client initialized) → `Poll::Ready(Ok(()))`, `semaphore.available_permits() == 128` (`DEFAULT_CONCURRENCY_LIMIT`). Command: `cargo test -p camel-component-opensearch --lib`. Expected: fails before, passes after.
- `call_closed_semaphore_returns_error`: `semaphore.close()`  then drive `call` future → returns `Err(ConsumerStopping)` (acquisition failure surfaced in call). Command: same. Expected: passes after.
- `call_blocks_on_semaphore_until_release`: drain ALL 128 permits externally (`while let Ok(p) = Arc::clone(&producer.semaphore).try_acquire_owned() { held.push(p) }`), spawn `call` future, poll → Pending while drained; drop held permits → future proceeds past acquisition. Command: same. Expected: passes after.

**Acceptance:**
- `cargo test -p camel-component-opensearch --lib` passes.
- `cargo clippy -p camel-component-opensearch -- -D warnings` exits 0.

- [x] 2.6

### camel-component-grpc

#### Task 2.7: GrpcProducer — unconditional readiness

**Files:**
- `crates/components/camel-component-grpc/src/producer/mod.rs` (modified)

**Steps:**
1. Delete `pending_permit` field and `acquire_fut` (an `Option`-based `AcquireFut` alias, same shape as kafka/wasm) and their `Clone` entries; keep `semaphore`.
2. `poll_ready`: return `Poll::Ready(Ok(()))` unconditionally.
3. `call`: remove permit-take branch. Before `Box::pin`, clone the semaphore Arc into a local; inside the async block first `let _permit = semaphore.acquire_owned().await.map_err(|_| CamelError::ChannelClosed)?;` then existing request/deadline/retry logic unchanged. (Arc clone before pinning.)
4. Update in-file tests referencing permit readiness.

**Tests:**
- `poll_ready_returns_ok_unconditionally`: built producer (channel to a dummy endpoint is acceptable — construction pattern from existing tests in the file) → `Poll::Ready(Ok(()))`, `semaphore.available_permits() == 128` (`DEFAULT_CONCURRENCY`). Command: `cargo test -p camel-component-grpc --lib`. Expected: fails before, passes after.
- `call_closed_semaphore_returns_error`: `semaphore.close()`  then drive `call` future → returns `Err(ChannelClosed)` (acquisition failure surfaced in call). Command: same. Expected: passes after.
- `call_blocks_on_semaphore_until_release`: drain ALL 128 permits externally (`while let Ok(p) = Arc::clone(&producer.semaphore).try_acquire_owned() { held.push(p) }`), spawn `call` future, poll → Pending while drained; drop held permits → future proceeds past acquisition. Command: same. Expected: passes after.

**Acceptance:**
- `cargo test -p camel-component-grpc --lib` passes.
- `cargo clippy -p camel-component-grpc -- -D warnings` exits 0.

- [x] 2.7
