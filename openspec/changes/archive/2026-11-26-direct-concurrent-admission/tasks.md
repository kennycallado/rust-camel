# Tasks: direct-concurrent-admission

Single-phase change (no `## Phases` in design.md). Spec blessing: APPROVED
round 2 (`sha256:3a6fc0f333a22ad297ebf4047a8c23db70e8f73099ce940d9fc5854596f771eb`).
Plan blessing: APPROVED-WITH-FIXES round 1, all fixes applied, re-bless
APPROVED (`sha256:b4bf7323092e5c2b49b291ab7a53ae21e610d2c8f5e1f1a370883a2a3dde2484`,
see `.bless.json`). bd: rc-zy47o. Worktree:
`/home/shared/rust-camel-worktrees/direct-concurrent-admission`. Artifacts
(red/green logs) go to `/home/shared/tmp/direct-concurrent-admission/`.

Crate note: the camel-direct package name is `camel-component-direct`
(`crates/components/camel-direct/Cargo.toml:2`) — every cargo command below
uses `-p camel-component-direct`. All logged test commands run with
`set -o pipefail; ... 2>&1 | tee <file>` so the tee cannot mask a failure.

## camel-test (route-level tests)

### Task 1.1: Red route-level parallel-admission test

**Files:**
- `crates/camel-test/tests/direct_concurrent_admission_test.rs` (new)

**Steps:**
1. Create the test file with a module doc explaining it pins
   `direct-dispatch` "No producer-side serialization for Concurrent
   consumers". Declare an async helper `async fn resolve_producer(h:
   &CamelTestContext, uri: &str) -> BoxProcessor` that awaits
   `h.ctx().lock()`, reads `producer_context()` and `registry()`, gets the
   `"direct"` component, `create_endpoint(uri)`,
   `create_producer(Arc::new(camel_component_api::NoOpComponentContext),
   &producer_ctx)` — copy the inner block of `send_await_reply`
   (`crates/camel-test/tests/integration_test.rs:377-390`) WITHOUT the
   retry loop.
2. Write the test below. Runtime contract: `#[tokio::test]`
   (current-thread — REQUIRED for `with_time_control()`; a
   `multi_thread` flavor panics inside `tokio::time::pause()`).
3. Harness:
   `CamelTestContext::builder().with_component(DirectComponent::new()).with_mock().with_time_control().build().await`
   → `(h, _tc)`. Immediately `tokio::time::resume()`, then `add_route`,
   `h.start().await` — NO real sleep needed (the direct consumer
   registers in the registry before `mark_ready()`, so a completed
   `start()` implies a dispatchable entry). Then
   `tokio::time::pause()` before the measured phase. Use
   `tokio::time::Instant` (not `std`) for all elapsed measurement.
4. Route: `RouteBuilder::from("direct:work").route_id("admission-target").concurrent(2).process(BOTH_ENTER_STEP).to("mock:done").build().unwrap()`
   where `BOTH_ENTER_STEP` is a closure in the existing
   `.process(move |ex| { ... async move { ... } })` shape
   (`integration_test.rs:1875-1883`; `.process_fn` takes a
   `BoxProcessor`, not a closure); it increments
   `entered: Arc<AtomicUsize>`, then
   `tokio::time::sleep(Duration::from_millis(200)).await`, then
   `Ok(ex)`. Add the route, start the harness.
5. Measured phase: `let t0 = tokio::time::Instant::now();` resolve ONE
   producer `resolve_producer(&h, "direct:work").await`; make two
   clones `let p1 = producer.clone(); let p2 = producer.clone();`
   (BoxProcessor is `Clone`; the clones share the producer state — the
   exact one-`to:`-step shape; no `mut` bindings —
   `ServiceExt::oneshot` takes the service by value); `tokio::spawn`
   two oneshot futures
   (`tower::ServiceExt::oneshot(pX, Exchange::new(Message::new(format!("p{i}"))))`).
   Maintain `completed: Arc<AtomicUsize>` by incrementing it after each
   spawned task's result resolves (wrap each oneshot in a small task
   that stores the result and increments `completed`). Drive the
   spawned tasks with
   `for _ in 0..50 { tokio::task::yield_now().await; }`, then snapshot
   `let snap = (entered.load(SeqCst), completed.load(SeqCst));`.
6. Join both spawned tasks inside a virtual timeout — do NOT manually
   advance time: `let results = tokio::time::timeout(Duration::from_millis(500),
   async { (ra.await, rb.await) }).await.expect("both calls must
   complete within 500 ms virtual")` (tokio paused-time auto-advance
   drives the 200 ms sleeps while the runtime is idle on the join).
7. Assert, in this order: both replies are `Ok` with bodies `"p0"` and
   `"p1"`; `h.mock().get_endpoint("done").unwrap().assert_exchange_count(2).await`;
   then the overlap snapshot `assert_eq!(snap, (2, 0), "both 200 ms
   pipelines must be in flight and neither complete before either
   completes")`; then
   `assert!(t0.elapsed() < Duration::from_millis(350), "parallel calls
   must complete under 350 ms virtual, got {:?}", t0.elapsed())`.
8. Stop the harness (`h.stop().await`).
9. Run the RED run and RECORD it (pre-fix, the permit must make this
   fail at the snapshot): `mkdir -p
   /home/shared/tmp/direct-concurrent-admission && set -o pipefail &&
   cargo test -p camel-test --test
   direct_concurrent_admission_test concurrent_consumer_parallel_admission
   2>&1 | tee /home/shared/tmp/direct-concurrent-admission/red-concurrent-consumer-parallel-admission.log`.
   Accept RED ONLY if the same test reaches the snapshot assertion with
   `(1, 0)` — exactly one pipeline entered and zero completed. An
   observed `(0, 0)` does NOT prove serialization (find the yield-count
   or startup bug instead).

**Tests:**
- `concurrent_consumer_parallel_admission`: setup — harness with
  DirectComponent + mock + time control; route `direct:work` with
  `.concurrent(2)` and a 200 ms sleep step; one producer, two clones.
  action — spawn two oneshot calls, yield until parked, snapshot, join
  under a 500 ms virtual timeout. assert — snapshot `(2, 0)`; both
  replies Ok with original bodies; mock:done saw 2; elapsed < 350 ms
  virtual. command — `cargo test -p camel-test --test
  direct_concurrent_admission_test`. expected — RED pre-fix (snapshot
  `(1, 0)`, assert fails; log recorded), GREEN post-fix.

**Acceptance:**
- The red log exists at
  `/home/shared/tmp/direct-concurrent-admission/red-concurrent-consumer-parallel-admission.log`
  and shows the snapshot assertion failure with `(1, 0)`.
- `cargo fmt --check --all` exits 0 (worktree root).
- `cargo clippy -p camel-test --all-targets -- -D warnings` exits 0.
- Post-fix (Task 1.2), the same command is GREEN.

- [x] 1.1

## camel-component-direct (the fix)

### Task 1.2: Remove the producer admission permit

**Files:**
- `crates/components/camel-direct/src/lib.rs` (modified)
- `crates/components/camel-direct/src/direct_tests.rs` (modified)

**Steps:**
1. `lib.rs`: delete the `use tokio::sync::Semaphore;` import (line 18),
   the `semaphore: Arc::new(Semaphore::new(1))` construction in
   `create_producer` (line 272), the `semaphore` struct field
   (line 389), the clone arm (line 400), the
   `let semaphore = Arc::clone(&self.semaphore);` capture in `call`
   (line 456), and the channel-path acquire
   `let _permit = semaphore.acquire_owned().await...` (lines 517-520)
   so the `None` dispatcher arm is just
   `ctx.send_and_wait(exchange).await`.
2. `lib.rs` comment updates: the inline-path comment (lines 495-500)
   drops the "The endpoint semaphore is skipped" sentence and reads
   that the dispatcher's admission mutex is the single serializer for
   inline dispatches; the channel-path comment (lines 512-515) becomes
   "Channel path (fallback, not inline): submit through the consumer
   context. No producer-side permit: Concurrent routes parallelize per
   their `max` (per-envelope drain tasks); Sequential routes serialize
   in their single drain task in channel arrival order."; the
   `poll_ready` comment (lines 427-431) drops the permit wording but
   keeps the tower-contract rationale (poll_ready must not reserve
   resources); the timed-section comment (lines 468-471) drops
   "channel permit" and reads "the per-path serialization wait
   (dispatcher admission or channel wait)". Add the deadline caveat to
   the channel-path comment: "The producer deadline and error are
   unchanged. More calls can reach the bounded route queue before
   timing out. Once enqueued, an exchange can still execute after its
   caller times out."
3. `direct_tests.rs`: update the `direct_producer` helper (line 482) to
   drop the `semaphore` field. Delete the two tests whose premise is
   the permit: `call_blocks_on_semaphore_until_release` (lines
   613-690) and `call_pending_when_all_permits_held` (lines 692-731).
   Rewrite any remaining `producer.semaphore` reference (e.g. the
   poll_ready test near line 525) out without changing that test's own
   contract.
4. `direct_tests.rs`: remove the stale "distinct instance → distinct
   endpoint semaphore" comment in `inline_timeout_covers_admission_wait`
   (lines 1493-1496) — replace with "distinct instance; admission is
   the dispatcher mutex, not any producer state".
5. `direct_tests.rs`: rename `concurrent_producers_serialized_fifo`
   (line 1566) to `sequential_consumer_keeps_fifo` and make admission
   order deterministic per the blessed design. Do NOT `tokio::spawn`:
   create the four call futures locally —
   `let waker = noop_waker(); let mut cx = Context::from_waker(&waker);`
   then for `i` in `0..4`: `let mut fut =
   prod.clone().call(Exchange::new(Message::new(format!("p{i}"))));`
   and poll it ONCE: `assert!(fut.as_mut().poll(&mut
   cx).is_pending(), "first poll of p{i} must be Pending")` — the
   Pending first poll records the arrival order at the dispatcher's
   FIFO admission mutex before any dispatch completes. Collect the four
   futures in order, then drive them to completion with sequential
   awaits in the same order (`for fut in futs { let reply =
   fut.await.expect("p{i} must complete Ok"); }` — camel-direct has no
   `futures` dev-dep). Retain the paired begin/end no-interleaving
   checks; add the exact-order assertion
   `assert_eq!(seen, vec!["p0", "p1", "p2", "p3"], "admission must
   follow p0-p3 arrival order")`.
6. Verify no semaphore references remain:
   `grep -rni semaphore crates/components/camel-direct/src/` returns
   NOTHING (the fake dispatchers model the INLINE dispatcher's
   admission mutex with `tokio::sync::Mutex` and do not need the word).
7. Run the unit suite and the now-green integration test; record GREEN:
   `set -o pipefail && cargo test -p camel-component-direct 2>&1 | tee
   /home/shared/tmp/direct-concurrent-admission/green-camel-component-direct.log`
   and `set -o pipefail && cargo test -p camel-test --test
   direct_concurrent_admission_test concurrent_consumer_parallel_admission
   2>&1 | tee
   /home/shared/tmp/direct-concurrent-admission/green-concurrent-consumer-parallel-admission.log`.

**Tests:**
- `sequential_consumer_keeps_fifo`: setup — registry entry with a
  `FifoRecorderDispatcher` (admission mutex + event log); producer with
  2 s timeout. action — create 4 local clone-call futures, poll each
  once in p0→p3 order (each Pending), then await them sequentially.
  assert — every future completes `Ok`; 8 events; each begin
  immediately followed by its end; no duplicates; and
  `seen == ["p0","p1","p2","p3"]`. command — `cargo test -p
  camel-component-direct sequential_consumer_keeps_fifo`. expected —
  GREEN (unchanged mechanism, new explicit order assertion).
- `concurrent_consumer_parallel_admission` (from Task 1.1): command —
  `cargo test -p camel-test --test
  direct_concurrent_admission_test`. expected — GREEN post-fix.
- All other camel-component-direct tests: command — `cargo test -p
  camel-component-direct`. expected — GREEN (the two deleted permit
  tests are the only removals).
- camel-core direct-named tests: command — `cargo test -p camel-core
  direct`. expected — exit 0.

**Acceptance:**
- `cargo test -p camel-component-direct` exits 0.
- `cargo test -p camel-core direct` exits 0.
- `cargo clippy -p camel-component-direct --all-targets -- -D warnings`
  exits 0.
- `cargo fmt --check --all` exits 0.
- `grep -rni semaphore crates/components/camel-direct/src/` returns
  nothing.
- The green logs exist under `/home/shared/tmp/direct-concurrent-admission/`.

- [x] 1.2

## camel-test (route-level tests, post-fix)

### Task 1.3: Channel-path timeout parity test

**Files:**
- `crates/camel-test/tests/direct_concurrent_admission_test.rs` (modified)

**Steps:**
1. Add `direct_timeout_still_counts_admission_wait` to the same file.
   `#[tokio::test]` (current-thread), same resume-for-startup /
   pause-for-measured-phase pattern as Task 1.1 (no real sleep).
2. Harness: DirectComponent + mock + time control. Route:
   `RouteBuilder::from("direct:work").route_id("timeout-target").concurrent(1).process(PARK_P0_STEP).to("mock:done").build().unwrap()`
   where `PARK_P0_STEP` is a closure in the `.process(move |ex| { ...
   async move { ... } })` shape; it increments an
   `entered: Arc<AtomicUsize>`, pushes `"begin:{body}"` into an
   `events: Arc<Mutex<Vec<String>>>`, parks body `"p0"` on a
   `release: Arc<Notify>` (other bodies fall through), pushes
   `"end:{body}"`, returns `Ok(ex)`.
3. Resolve ONE producer `resolve_producer(&h,
   "direct:work?timeout_ms=100").await` — the short timeout lives on
   the producer config, so every clone shares it. Create p0's local
   clone-call future (`let mut f0 = p0.clone().call(...p0...)`), poll
   it ONCE with a `noop_waker` context, then drive the runtime with
   bounded yields (`for _ in 0..50 { yield_now().await; }`) until the
   events log shows `["begin:p0"]` (the route's drain task parks p0 in
   the pipeline). Leave f0 UNPOLLED — its caller future stays alive and
   pending, which is what held the old shared permit pre-fix; keeping
   it unpolled is what makes the post-release assertions RED pre-fix.
4. Await p1 through another clone INSIDE a virtual timeout:
   `let p1_result = tokio::time::timeout(Duration::from_millis(300),
   p1.clone().call(...p1...)).await.expect("p1 must resolve within 300
   ms virtual")` — p1's own 100 ms producer timeout fires first.
   Assert p1 failed with the canonical text:
   `matches!(&p1_result, Err(CamelError::ProcessorError(m)) if m ==
   "direct:work call timed out")` and the events log is still exactly
   `["begin:p0"]` (only the first exchange entered the pipeline while
   p1 waited).
5. Poll f0 to completion (its own 100 ms timeout also fires): poll/await
   f0 under a bounded virtual timeout and assert it resolves to
   `Err(ProcessorError)` with the same canonical text (the caller
   deadline expired; the parked envelope is NOT retracted).
6. Release: `release.notify_one();` then
   `h.mock().get_endpoint("done").unwrap().await_exchanges(2,
   Duration::from_secs(1)).await;` — under paused time the mock wait
   and the route wake drive by auto-advance; then assert exactly two
   receipts (`assert_exchange_count(2)`) and the full trace
   `["begin:p0", "end:p0", "begin:p1", "end:p1"]` — the enqueued
   exchange executed after its caller timed out.
7. Stop the harness.

**Tests:**
- `direct_timeout_still_counts_admission_wait`: setup — Concurrent
  route `direct:work` at `.concurrent(1)` with a park-on-first step;
  ONE producer at `timeout_ms=100`, two clones. action — poll p0 once,
  observe `begin:p0`, await p1 to its timeout, poll p0 to its timeout,
  release, await two mock receipts. assert — p1 and p0 resolve
  `Err(ProcessorError("direct:work call timed out"))`; trace is
  `["begin:p0"]` at p1 failure; after release the trace is
  `["begin:p0","end:p0","begin:p1","end:p1"]` with 2 mock receipts.
  command — `cargo test -p camel-test --test
  direct_concurrent_admission_test direct_timeout_still_counts_admission_wait`.
  expected — GREEN post-fix (pre-fix RED: p1's envelope is never
  submitted — it dies on the permit — so the post-release trace lacks
  p1's entries).

**Acceptance:**
- The named test passes: `cargo test -p camel-test --test
  direct_concurrent_admission_test` exits 0 (both route-level tests so
  far).
- `cargo fmt --check --all` and `cargo clippy -p camel-test
  --all-targets -- -D warnings` exit 0.

- [x] 1.3

### Task 1.4: Sequential channel-arrival-order test

**Files:**
- `crates/camel-test/tests/direct_concurrent_admission_test.rs` (modified)

**Steps:**
1. Add `sequential_channel_consumer_keeps_fifo` to the same file.
   `#[tokio::test]`, same time-control pattern.
2. Harness: DirectComponent + mock + time control. Route (Sequential by
   default; the top-level aggregate with a timeout condition compiles to
   the aggregate split — trigger verified at
   `crates/camel-core/src/lifecycle/adapters/route_helpers.rs:180-192`:
   `has_timeout_condition(config.completion)`):
   `RouteBuilder::from("direct:agg").route_id("agg-split-target").process(TRACE_PARK_STEP).aggregate(AggregatorConfig::correlate_by("id").complete_on_timeout(Duration::from_millis(500)).build().expect("valid aggregator config")).to("mock:agg-out").build().unwrap()`
   (import `camel_api::aggregator::AggregatorConfig`; mirror
   `integration_test.rs:1888-1895` for builder shape; `TRACE_PARK_STEP`
   is a closure in the `.process(move |ex| { ... async move { ... } })`
   shape). EVERY submitted exchange MUST set the correlation header:
   `ex.input.headers.insert("id".to_string(), serde_json::json!("bucket-a"))`
   (same key for all three, mirroring `integration_test.rs:1877-1883`)
   — the aggregator rejects exchanges missing the header with
   `ProcessorError("Aggregator: missing correlation key header 'id'")`
   (`crates/camel-processor/src/aggregator.rs:898-910`), which would
   leave mock:agg-out empty.
   The `TRACE_PARK_STEP` closure: increments
   `entered: Arc<AtomicUsize>`, pushes `"begin:{body}"`, parks the
   FIRST body `"p0"` on a `release: Arc<Notify>`, pushes
   `"end:{body}"`, returns `Ok(ex)`.
3. Resolve ONE producer `resolve_producer(&h, "direct:agg").await`; make
   three clones. Create p0's local clone-call future, poll it ONCE,
   yield until `entered == 1`. Then create p1's and p2's local
   clone-call futures and poll each ONCE, in that order (p1 first, then
   p2 — spawn/poll order is not implicitly arrival order; the explicit
   poll order ESTABLISHES channel arrival order per
   `design.md:64-70,124-126`); assert both first polls returned
   `Pending` (each must await a result variable — collect the futures
   in a `Vec` for the later join).
4. Release p0 (`release.notify_one()`), then join all three reply
   results inside a bounded virtual timeout
   (`tokio::time::timeout(Duration::from_millis(500), async { ... all
   three futures ... })`) and require every reply to succeed — any
   `Err` (including the aggregator's missing-correlation-key error) is
   reported in the failure message. Assert the exact pre-stage trace:
   `assert_eq!(*events.lock().unwrap(), vec!["begin:p0", "end:p0",
   "begin:p1", "end:p1", "begin:p2", "end:p2"])` — one-at-a-time
   execution in channel arrival order, no interleaving.
5. Replace any single large advance with the bounded mock wait:
   `h.mock().get_endpoint("agg-out").unwrap().await_exchanges(1,
   Duration::from_secs(1)).await;` (paused-time auto-advance drives the
   500 ms completion timeout while the wait polls idle), then assert
   exactly one receipt
   (`assert_exchange_count(1)`). Stop the harness.
6. Contingency: if the split does not trigger (route compiles as
   in-branch aggregate and events interleave differently), OR a reply
   carries the aggregator's missing-correlation-key error, STOP and
   report `test-design-gap: <which>` — do not silently switch to a
   different route shape.

**Tests:**
- `sequential_channel_consumer_keeps_fifo`: setup — Sequential
  aggregate-split route `direct:agg` (completion timeout 500 ms), trace
  + park-first pre-step with correlation header `id` on every exchange;
  one producer, three clones. action — poll p0 once, observe
  `begin:p0`, poll p1/p2 once each in order (both Pending), release,
  join all replies bounded, await one mock receipt. assert — every
  reply Ok; exact trace
  `begin:p0,end:p0,begin:p1,end:p1,begin:p2,end:p2`; mock:agg-out sees
  exactly 1 aggregated exchange. command — `cargo test -p camel-test
  --test direct_concurrent_admission_test
  sequential_channel_consumer_keeps_fifo`. expected — GREEN post-fix.

**Acceptance:**
- `cargo test -p camel-test --test direct_concurrent_admission_test`
  exits 0 (all three route-level tests).
- `cargo fmt --check --all` and `cargo clippy -p camel-test
  --all-targets -- -D warnings` exit 0.

- [x] 1.4

## docs

### Task 1.5: Update direct component docs

**Files:**
- `docs/src/components/direct.md` (modified)

**Steps:**
1. `docs/src/components/direct.md` line 97 paragraph: rewrite to the
   permit-free channel path. Keep ASD-STE100 style (short sentences,
   active voice). Content: `to: "direct:name"` builds a DirectProducer.
   Inline dispatch is the primary path: for a Sequential consumer the
   producer runs the pipeline on its own task under the dispatcher's
   FIFO admission. Channel submission is the fallback, not the inline
   path: the producer submits the exchange to the consumer's channel
   and awaits the reply; it holds no admission permit. On the channel
   path a Concurrent route processes exchanges in parallel up to its
   `max`; a Sequential route processes them one at a time in arrival
   order. `poll_ready` checks only the registry; it fails fast when no
   Consumer is registered and `failIfNoConsumers` is not `false`.
   Carry the caveat verbatim: "The producer deadline and error are
   unchanged. More calls can reach the bounded route queue before
   timing out. Once enqueued, an exchange can still execute after its
   caller times out." (Release notes come from the fix commit message;
   there is no hand-edited CHANGELOG file in this repo.)
2. Scan the rest of `direct.md` and the route-concurrency doc page (if
   one exists under `docs/src/`) for other claims that a direct
   producer serializes calls or holds "one in-flight call"; fix any
   such sentence the same way.
3. Run the docs tests: `cd docs && mdbook test` (mdbook is available
   locally; if the binary is absent, report that instead of skipping
   silently).
4. Run the local gate sweep (merge precondition):
   `bash scripts/gates-local.sh` from the worktree root and record the
   exit code.

**Tests:**
- No new Rust tests. Verification: `cd docs && mdbook test` exits 0.
  command — `cd docs && mdbook test`. expected — exit 0.

**Acceptance:**
- `grep -n "semaphore" docs/src/components/direct.md` returns nothing.
- `grep -n "one in-flight call" docs/src/components/direct.md` returns
  nothing.
- `cd docs && mdbook test` exits 0 (or the mdbook-absent report).
- `bash scripts/gates-local.sh` exit code recorded in the task report.

- [x] 1.5
