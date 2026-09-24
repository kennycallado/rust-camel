# Tasks: settlefix

## camel-api / workspace gauge

### Task 1.1: InFlightGauge with zero-transition notification + workspace type swap

**Files:**
- `crates/camel-api/src/in_flight.rs` (modified)
- `crates/camel-api/src/exchange.rs` (modified — doc comments referencing the raw counter type)
- `crates/camel-api/Cargo.toml` (modified — ensure `tokio` dep with `sync` feature)
- `crates/camel-core/src/context.rs` (modified)
- `crates/camel-core/src/context_builder.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_registry.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_ext.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/inline_dispatcher.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/controller_component_context.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_drainclaim_tests.rs` (modified — direct `AtomicU64` loads become gauge `total()` reads)
- `crates/camel-core/src/lifecycle/adapters/inline_dispatcher_tests.rs` (modified — helper takes `&AtomicU64` today; becomes gauge `total()` reads)
- `crates/components/camel-component-api/src/component_context.rs` (modified)
- `crates/components/camel-component-api/src/consumer.rs` (modified)
- `crates/components/camel-component-api/src/runtime_observability.rs` (modified)
- `crates/components/camel-component-api/src/consumer_claim_tests.rs` (modified — gauge construction + attach adaptation)
- `crates/components/camel-component-seda/src/lib.rs` (modified — type swap only)
- `crates/components/camel-component-ws/src/client_consumer.rs` (modified — type swap only)
- `crates/components/camel-component-ws/src/client_consumer_tests.rs` (modified — gauge adaptation)
- `crates/components/camel-component-grpc/src/consumer.rs` (modified — type swap only)
- `crates/components/camel-component-grpc/tests/integration.rs` (modified — gauge adaptation)
- `crates/components/camel-http/src/lib.rs` (modified — type swap only)
- `crates/components/camel-master/src/leadership.rs` (modified — type swap only)
- `crates/components/camel-master/src/supervision.rs` (modified — type swap only)
- `crates/camel-processor/src/aggregator.rs` (modified — claim-mint sites, type swap only)
- `crates/camel-processor/src/resequencer/mod.rs` (modified — claim-mint sites, type swap only)
- `crates/camel-cli/src/commands/job/batch.rs` (modified — test adapts to gauge)

**Steps:**
1. In `crates/camel-api/src/in_flight.rs` introduce
   `pub struct InFlightGauge { count: AtomicU64, idle: tokio::sync::Notify }`
   with `pub fn new() -> Self` plus `impl Default` (clippy
   `new_without_default`), `pub fn total(&self) -> u64` (Acquire load), and
   `pub fn idle(&self) -> &tokio::sync::Notify`. Increment/decrement are
   crate-private (`pub(crate) fn inc`, `pub(crate) fn dec`); `dec` performs
   `fetch_sub(1, Release)` and when the previous value was 1 (last release)
   calls `self.idle.notify_waiters()`.
2. Change `InFlightClaim(Arc<AtomicU64>)` to `InFlightClaim(Arc<InFlightGauge>)`;
   `InFlightClaim::attach(gauge: &Arc<InFlightGauge>) -> Self` increments;
   `InFlightClaim::split` (in_flight.rs:49-52) also switches its `fetch_add`
   to `inc()`; `impl Drop` decrements via `dec` (so the last release
   notifies waiters). Keep the `Debug` impl. Keep `ExchangeEnvelope`'s
   `in_flight_claim: Option<InFlightClaim>` field unchanged.
3. Swap every remaining in-flight counter handle
   `Arc<std::sync::atomic::AtomicU64>` → `Arc<InFlightGauge>` at the files
   listed above: `CamelContext` fields and `total_in_flight()` (now
   `self.in_flight_total.total()`), `context_builder.rs` construction
   (`Arc::new(InFlightGauge::new())`) and `set_in_flight_total`, the six
   lifecycle adapter files, the `in_flight_counter()` trait method in
   camel-component-api (`component_context.rs`, `runtime_observability.rs`)
   and `consumer.rs`, the stored `in_flight` fields and mint sites in
   camel-component-seda/-ws/-grpc, camel-http, camel-master, the
   aggregator/resequencer claim-mint sites, and the `batch.rs` test's
   `in_flight_counter()` usage. Claim minting stays
   `InFlightClaim::attach(&gauge)`; test adaptations construct gauges with
   `InFlightGauge::new()` and read via `total()` — `inc`/`dec` stay
   camel-api-private, so NO cross-crate `fetch_*` arithmetic remains on
   gauge handles.
4. Add `pub fn in_flight_gauge(&self) -> Arc<InFlightGauge>` to
   `CamelContext` in `crates/camel-core/src/context.rs` (runner seam;
   returns a clone of the shared gauge Arc).
5. Run `cargo build --workspace` in the worktree; fix every mechanical
   compile error until green.

**Tests:** (executable spec — name, arrange, act, assert)
- `gauge_counts_claim_lifecycle` (in_flight.rs tests): `InFlightGauge::new()`, attach two claims → `total() == 2`; drop one → `total() == 1`; drop both → `total() == 0`.
- `gauge_notifies_on_last_release_only` (tokio::test, in_flight.rs tests): pin+`enable()` a `gauge.idle().notified()` future; attach two claims; drop the first → the enabled future must NOT resolve within 100ms (`tokio::time::timeout` elapses); drop the second → the future resolves (await with 1s timeout succeeds).
- `gauge_notify_wakes_all_enabled_waiters` (tokio::test): two separately enabled `notified()` futures; last release resolves both.
- `cargo test -p camel-api --lib` — all existing in_flight tests pass (adapted to the gauge API).

**Acceptance:**
- `cargo build --workspace` exits 0 in the worktree.
- `cargo test -p camel-api --lib` passes; `cargo test -p camel-core --lib` passes; `cargo test -p camel-component-api --lib` passes; `cargo test -p camel-component-ws --lib` passes.
- `cargo check -p camel-component-grpc --tests` exits 0.
- `cargo clippy -p camel-api -p camel-core -p camel-component-api -p camel-component-seda -p camel-component-ws -p camel-component-grpc -p camel-http -p camel-master -p camel-processor -- -D warnings` exits 0.
- `grep -rn 'in_flight' crates --include='*.rs' | grep 'Arc<AtomicU64>'` returns no in-flight counter hits (unrelated AtomicU64 uses outside in-flight plumbing may remain).

- [x] 1.1

## camel-mock

### Task 2.1: Component-level arrival notification slots

**Files:**
- `crates/components/camel-mock/src/lib.rs` (modified)
- `crates/components/camel-mock/src/inner.rs` (modified — record-site ping)

**Steps:**
1. Add to `MockComponent` a clone-shared slot registry
   `arrival_slots: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Notify>>>>`
   — `Arc`-wrapped because `MockComponent` is `#[derive(Clone)]` and the
   runner registers via a clone while the registered component records via
   another; constructed in `with_config`/`new` alongside the existing
   endpoint registry.
2. Add `pub fn ensure_arrival_notify(&self, name: &str) -> Arc<tokio::sync::Notify>`:
   returns the existing slot for `name` or inserts a fresh one — the slot
   exists BEFORE the endpoint is created (`get_endpoint` returns `None`
   until first receive; registration must not depend on endpoint existence).
3. Thread the ping to the record site explicitly: `MockComponent` carries
   the clone-shared slot registry; `create_endpoint` (lib.rs, has `&self` +
   name) clones the registry Arc into the new `MockEndpointInner`, which in
   turn hands it to its `MockProducer`; the record path in `inner.rs` (the
   `push_back` record site, which owns a `MockProducer` — not a component
   handle) pings the slot for its endpoint name on that registry after the
   exchange is recorded, including the endpoint auto-creation receive. The
   ping helper is a small function over the shared registry
   (`fn ping(slots: &Arc<Mutex<HashMap<String, Arc<Notify>>>>, name: &str)`),
   not a `MockComponent` method. Note: `MockComponent::new()` takes no
   args in the current code.
4. Run the camel-mock crate's test suite and the camel-cli
   document/driver suites that exercise mock endpoints to confirm no
   behavior change.

**Tests:**
- `arrival_slot_available_before_endpoint` (camel-mock tests): fresh `MockComponent`; `ensure_arrival_notify("result")` returns a slot; `get_endpoint("result")` is still `None` (slot independent of endpoint creation).
- `arrival_slots_shared_across_clones` (camel-mock tests): `let a = MockComponent::new(); let b = a.clone();` slot from `a.ensure_arrival_notify("result")` is the same `Arc` (pointer-equal) as `b.ensure_arrival_notify("result")`.
- `receive_pings_arrival_slot` (tokio::test, camel-mock tests): `ensure_arrival_notify("result")`; pin+enable a `notified()` future; deliver one exchange to `mock:result` through the component's receive path (however the existing tests drive mock receive); the future resolves.
- `receive_without_prior_ensure_creates_slot_for_late_registration` (camel-mock tests): deliver one exchange first; `ensure_arrival_notify("result")` succeeds afterwards (slot auto-created by ping; next receive resolves an enabled waiter).

**Acceptance:**
- `cargo test -p camel-component-mock` (or the crate's actual test target) passes including the four new tests.
- `cargo clippy -p camel-component-mock -- -D warnings` exits 0.
- Existing camel-mock and camel-cli mock-endpoint tests pass unchanged.

- [x] 2.1

## camel-cli runner

### Task 3.1: Notification-based settle (mode split, sampler deleted)

**Files:**
- `crates/camel-cli/src/commands/test/runner.rs` (modified)

**Steps:**
1. Delete the `SAMPLE_INTERVAL` constant and the sampling loop in `settle`.
   Keep `SETTLE_DEADLINE` (5s) and `DEFAULT_QUIET` (250ms).
2. Add `fn has_self_firing_consumer(defs: &[camel_core::RouteDefinition]) -> bool`
   derived from the same `from:` URI scan the runner already performs for
   seda consumer names: true when any route's from-URI starts with
   `timer:` (the only self-firing source in the lean registry).
3. Add
   `async fn settle_completion(gauge: &Arc<InFlightGauge>, timeout: Duration, settle_entry: Instant) -> Result<(), String>`:
   - deadline = settle_entry + timeout (anchored at settle entry — after
     input delivery — so delivery time never consumes the settle budget).
   - Loop: FIRST check the deadline — `Instant::now() >= deadline` → `Err`
     (checked before any idle acceptance, including the very first
     iteration and every notification wake; a release racing the deadline
     resolves too late). Then pin+`enable()` `gauge.idle().notified()`; if
     `gauge.total() == 0` → `Ok(())`; `select!` the enabled future against
     `sleep_until(deadline)`; on notify wake → loop (re-check deadline,
     re-enable, re-check idle).
   - Error text: `"settle timeout: no completion notification within {timeout:?}"`
     (keeps the `settle timeout:` prefix).
4. Add
   `async fn settle_stability(mock: &MockComponent, names: &[String], quiet: Duration, route_started_at: Instant) -> Result<(), String>`:
   - deadline = route_started_at + quiet + SETTLE_DEADLINE (unchanged formula).
   - Slots: `ensure_arrival_notify` for every name in `names` (including
     not-yet-created endpoints).
   - last_counts = `sample_counts(mock, names)`, last_change = settle entry.
   - Loop: enable all slot `notified()` futures; sample counts; on change →
     update last_counts, last_change = now; then `select!` over the enabled
     futures and `sleep_until(min(last_change + quiet, deadline))`.
     On window expiry: if `now >= deadline` → `Err`; else re-sample
     (backstop) — changed → reset window; unchanged and
     `now - last_change >= quiet` → `Ok(())`.
   - Error text: keep the existing
     `"settle timeout: traffic did not quiesce within the 5s instability budget"`.
5. Rewire `run_phases`: capture `settle_entry` after input delivery;
   mode = `has_self_firing_consumer(defs)`; completion mode calls
   `settle_completion(&ctx.in_flight_gauge(), doc.settle_duration().unwrap_or(SETTLE_DEADLINE), settle_entry)`
   — stability mode calls
   `settle_stability(mock, &names, doc.settle_duration().unwrap_or(DEFAULT_QUIET), route_started_at)`
   with the existing expected-endpoint `names` collection. Only expected
   endpoints' counts participate in stability mode (exactly today's rule).
6. `sample_counts` stays as-is (used by `settle_stability` on wake).

**Tests:** (runner.rs `#[cfg(test)]`, mirroring `job/batch.rs` test style with the real gauge)
- `settle_completion_returns_immediately_when_idle` (tokio::test): `InFlightGauge::new()` idle; `settle_completion(&gauge, 5s, Instant::now())` returns `Ok` with elapsed < 50ms (no floor).
- `settle_completion_completes_on_last_release` (tokio::test multi_thread): attach a claim; spawn `settle_completion(&gauge, 5s, Instant::now())`; wait 10ms via a bounded wait helper (not a bare `sleep` — `lint-test-sleep` compliant pattern or an explicit `allow-test-sleep` marker); drop the claim; the spawned task returns `Ok` within 500ms (an old-style quiet window would exceed 250ms).
- `settle_completion_times_out_on_stuck_claim` (tokio::test): attach a claim and keep it alive; `settle_completion(&gauge, 50ms, Instant::now())` returns `Err` whose text starts with `settle timeout:` and elapsed < 1s (never-hang pin).
- `settle_completion_expired_deadline_immediate_err` (tokio::test): idle gauge but `settle_entry` set 10s in the past with timeout 5s → `Err` immediately (deadline already passed; never hang).
- `settle_stability_expired_deadline_immediate_err` (tokio::test): `MockComponent` with one expected name, `route_started_at` set 10s in the past (deadline = quiet + 5s long past) → `settle_stability` returns `Err` with the instability-budget message immediately, elapsed < 1s (deadline branch, never hang, without a 5s wait).
- `settle_stability_deadline_fires_while_emitting` (tokio::test multi_thread): `MockComponent` with expected name `result`; spawn a pinger task that delivers an exchange to `mock:result` every 50ms (each arrival resets the window); call `settle_stability` with `route_started_at = now - (quiet + Duration::from_millis(4900))` so the deadline lands ~100ms out; assert `Err` with the instability-budget message and elapsed < 1s (spec scenario "unstable traffic hits the deadline" — emitting arm, no 5s wait).
- `settle_completion_error_maps_to_settle_result` (tokio::test, runner module): hold a claim live on a booted context's gauge; run the settle phase expecting `settle_completion` to Err; feed the error through the SAME conversion `run_phases` uses for settle failures (the `<settle>` endpoint-result construction in runner.rs ~L655-662) and assert it produces a settle endpoint result that the driver counts as document failure (exit-1 path). The driver-level exit-1 pin is `timer_unstable_traffic_times_out_exit_1` in task 4.1.
- `has_self_firing_consumer_classifies_from_uris` (plain test): construct route definitions (or reuse the runner's URI-scan helper) — `timer:tick` from-URI → true; `direct:in`, `seda:q`, `mock:x` from-URIs → false.
- `settle_stability_window_resets_on_arrival` (tokio::test): `MockComponent` with expected name `result`; `settle: 200ms` equivalent quiet; deliver one exchange (receive path) 50ms in (bounded wait or `allow-test-sleep` marker); then none — settles `Ok` only after ~250ms total (window restarted at the change), assert elapsed ≥ 200ms and < 2s.

**Acceptance:**
- `cargo test -p camel-cli --lib` passes (all new runner tests green).
- `grep -n 'SAMPLE_INTERVAL' crates/camel-cli/src/commands/test/runner.rs` returns nothing.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo xtask lint-test-sleep` exits 0.

- [x] 3.1

## camel-cli driver coverage

### Task 4.1: Driver-level scenario tests (spec scenarios pinned)

**Files:**
- `crates/camel-cli/src/commands/test/driver_tests.rs` (modified)

**Steps:**
1. Survey `driver_tests.rs` for existing timer-settle, settle-timeout,
   multi-doc-order, and `maxCount: 0` coverage before writing new tests —
   reuse/extend existing tests where they already pin a scenario; add only
   what is missing.
2. Add the tests below following the file's existing helpers
   (`temp_dir`, route/doc writers, `run_tests_full`, `CleanupPaths`).
3. Ensure every test cleans temp paths via the existing `CleanupPaths`
   guard pattern.

**Tests:** (executable spec — name, arrange, act, assert)
- `lean_batch_settles_without_window_floor` (tokio::test): 5 lean documents (route `direct:in → mock:result`, one input, `expects count: 1` each, no `settle:`); `run_tests_full`; assert all pass AND total elapsed < 1250ms — the old algorithm's settle alone (5 × 250ms) exceeds this bound, so it pins the fast path (spec scenario "lean document settles on the completion notification").
- `settle_50ms_keeps_working_as_deadline` (tokio::test): one lean document with `settle: 50ms`; passes with elapsed < 1s (spec: existing config keeps working as a pure deadline).
- `timer_route_settles_before_assertion` (tokio::test): route `timer:tick?period=20ms&repeatCount=3 → mock:result`, no inputs, `expects: {mock:result: {count: 3}}`; passes (spec scenario "timer route settles before assertion"; stability window over arrival notifications).
- `timer_count_change_resets_window` (tokio::test): route `timer:tick?period=150ms&repeatCount=2 → mock:result`, `expects count: 2`, default quiet 250ms; passes — with the window resetting on each arrival the run cannot complete before the second fire + quiet; assert pass and elapsed ≥ 400ms (first fire ~0ms, second ~150ms, quiet 250ms → ≥ 400ms; old sampler would also pass — this pins the reset semantics, spec scenario "count change resets the quiet window").
- `maxcount_zero_absence_passes` (tokio::test): lean document with an extra `expects: {mock:silent: {maxCount: 0}}` where a route REFERENCES `mock:silent` but a filter blocks delivery (reuse the existing `max_count_zero_asserts_absence` test's `write_absence_doc` topology — the endpoint must be route-created or evaluation hard-fails "not created by any route"); extend the existing test rather than duplicating if it already covers both arms, and record the extended name (revised spec scenario "maxCount zero asserts absence after settling").
- `maxcount_zero_late_arrival_fails` (tokio::test): route `timer:late?period=1s&repeatCount=1&delay=100ms → mock:silent`, `expects: {mock:silent: {maxCount: 0}}`; stability mode settles after the 100ms arrival + quiet; evaluation FAILS with the at-most error text and exit code 1 (revised spec scenario, late-arrival failure arm).
- `timer_unstable_traffic_times_out_exit_1` (tokio::test): route `timer:flood?period=5ms` (no repeatCount — infinite) → `mock:result`, `expects: {mock:result: {count: 1}}`, default quiet 250ms; continuous arrivals reset the window until the deadline (quiet + 5s budget ≈ 5.25s — the fixed SETTLE_DEADLINE makes this an intentionally ~5s test; comment it); assert `summary.exit_code == 1` and STDOUT contains the `<settle>` FAIL line with the timeout text (the driver prints failed endpoint results to stdout, not stderr — runner.rs ~L655-662, test.rs ~L625-634).
- `multi_doc_execution_order_unchanged` (tokio::test): three lean documents with distinct endpoint names, passed in a known argument order; assert the per-document result lines appear in stdout in the same order (spec: sequential execution unchanged). If an existing test already pins ordering, extend it instead and record the name here.

**Acceptance:**
- `cargo test -p camel-cli --lib` passes with all new tests.
- Every added test name above exists and passes (or an existing test it extends is named in the task result).
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo xtask lint-test-sleep` exits 0.

- [x] 4.1

## docs

### Task 5.1: Settle semantics documentation

**Files:**
- `docs/src/testing/index.md` (modified)

**Steps:**
1. Locate the section documenting `settle` in `docs/src/testing/index.md`
   (grep `settle`).
2. Rewrite it to the new contract: documents whose routes have no
   self-firing (timer) consumer settle on the completion notification —
   `settle:` is the settle timeout (default 5s) anchored after input
   delivery; documents with timer consumers keep the quiet-window reading
   (`settle:` override, default 250ms; deadline = window + 5s budget).
3. Add a short historical note: the interim `settle: 50ms` mitigation
   (4.9x on lean batches) is superseded — lean documents now complete in
   microseconds without tuning; existing `settle: 50ms` configurations
   keep working as deadlines.

**Tests:**
- `settle_docs_consistency` (manual/CI grep check, not a Rust test): `grep -n 'settle' docs/src/testing/index.md` shows no remaining quiet-window-only description for completion-mode documents; the deadline reading is stated; the superseded-mitigation note exists.

**Acceptance:**
- `docs/src/testing/index.md` settle section matches the blessed spec semantics (completion-mode deadline; stability-mode window; superseded note).
- `cargo xtask lint-context-citations` exits 0 (no doc-lint regressions).

- [x] 5.1
