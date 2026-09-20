# Tasks: drainclaim

All cargo commands run with `RUSTC_WRAPPER=` (empty) and inside the
worktree `/home/shared/rust-camel-worktrees/drainclaim`.

## Phase 1: canonical claim primitive

### camel-component-api

#### Task 1.1: `InFlightClaim` type, envelope field, ConsumerContext attach, context accessor

**Files:**
- `crates/components/camel-component-api/src/consumer.rs` (modified)
- `crates/components/camel-component-api/src/component_context.rs` (modified)
- `crates/components/camel-component-api/src/runtime_observability.rs` (modified)
- `crates/components/camel-component-api/src/lib.rs` (modified, re-export only)

**Steps:**
1. In `consumer.rs`, add `pub struct InFlightClaim(Arc<AtomicU64>)` below
   `ExchangeEnvelope` with: `pub fn attach(counter: &Arc<AtomicU64>) -> Self`
   (does `fetch_add(1, AcqRel)`), `pub fn split(&self) -> Self` (does
   `fetch_add(1, AcqRel)` on the same counter and returns a sibling), and a
   `Drop` impl doing `fetch_sub(1, AcqRel)`. Document: one claim = one
   accepted-not-completed unit; fanout splits; RAII releases on every drop.
2. Add field `pub in_flight_claim: Option<InFlightClaim>` to
   `ExchangeEnvelope` (consumer.rs:19). Keep the struct non-`Clone`.
3. Add field `in_flight: Option<Arc<AtomicU64>>` to `ConsumerContext` and a
   builder `pub fn with_in_flight_counter(self, counter: Arc<AtomicU64>) -> Self`
   (same style as `with_startup`). In `send` (consumer.rs:316) and
   `send_and_wait` (consumer.rs:330), construct the envelope with
   `in_flight_claim: self.in_flight.as_ref().map(InFlightClaim::attach)`.
   Push failure drops the envelope, which drops the claim (rollback) —
   no manual rollback code.
4. In `component_context.rs`, add trait method
   `fn in_flight_counter(&self) -> Option<std::sync::Arc<std::sync::atomic::AtomicU64>> { None }`
   to `ComponentContext` with a doc comment: production contexts return the
   context-global accepted-not-completed counter; default `None` keeps test
   contexts uncounted.
5. In `runtime_observability.rs`, add the same-shaped default-`None` method
   `fn in_flight_counter(&self) -> Option<Arc<AtomicU64>> { None }` to
   `RuntimeObservability`, and forward it in the blanket
   `impl<T: ComponentContext> RuntimeObservability for T` as
   `<Self as ComponentContext>::in_flight_counter(self)` (mirror how
   `component_metrics` forwards).
6. Re-export `InFlightClaim` from `lib.rs` next to `ExchangeEnvelope`.
7. Fix in-crate envelope literals/patterns (consumer.rs tests,
   `dispatch.rs` tests) with `in_flight_claim: None` / `..`.

**Tests:** (executable spec)
- `claim_attach_increments_and_drop_decrements`: fresh `Arc<AtomicU64>` at 0 → `InFlightClaim::attach` → counter reads 1 → drop claim → reads 0. `RUSTC_WRAPPER= cargo test -p camel-component-api claim_ --lib`
- `claim_split_adds_one_sibling`: one claim live (counter 1) → `split()` → counter 2 → drop sibling → 1 → drop original → 0.
- `send_attaches_claim_when_counter_installed`: `ConsumerContext::new` + `with_in_flight_counter` + a receiving `mpsc` channel → `send(exchange)` → assert the received envelope's `in_flight_claim.is_some()` and the counter reads 1; drop envelope → 0.
- `send_without_counter_carries_none`: same setup without `with_in_flight_counter` → received envelope `in_flight_claim.is_none()`, counter untouched.
- `push_failure_rolls_claim_back`: `with_in_flight_counter` + a channel whose receiver is dropped → `send` returns `Err(ChannelClosed)` → counter reads 0.
- `raw_sender_path_stays_uncounted`: `with_in_flight_counter` + `ctx.sender()` → push `ExchangeEnvelope { exchange, reply_tx: None, in_flight_claim: None }` → counter stays 0.
- Expected before implementation steps 1-3: these tests fail to compile (no `InFlightClaim`, no field) — that is the red state.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-component-api --lib` exits 0.
- `RUSTC_WRAPPER= cargo clippy -p camel-component-api -- -D warnings` exits 0.

- [x] 1.1

#### Task 1.2: mechanical compile-compat sweep for the new envelope field

**Files:**
- `crates/camel-bench/benches/direct_decompose.rs` (modified)
- `crates/components/camel-component-grpc/src/consumer.rs` (modified)
- `crates/components/camel-http/src/lib.rs` (modified, mechanical)
- `crates/components/camel-ws/src/client_consumer.rs` (modified, test sites only)
- `crates/components/camel-ws/src/lib.rs` (modified, test sites only)
- `crates/components/camel-master/src/leadership.rs` (modified, test sites only)
- `crates/components/camel-direct/src/direct_tests.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified, temporary)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified, temporary)
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified)

**Steps:**
1. Add `in_flight_claim: None` to every `ExchangeEnvelope { .. }` literal in
   the listed files (construction sites: bench direct_decompose.rs:41,
   grpc consumer.rs 695/792/872/908/1032, http lib.rs:2054, ws
   client_consumer.rs:473 + lib.rs:1174/1220, master leadership.rs:406/407,
   direct direct_tests.rs:286 area, core route_controller_tests.rs sites).
2. Change every exhaustive destructure
   `let ExchangeEnvelope { exchange, reply_tx } = envelope;` to
   `let ExchangeEnvelope { exchange, reply_tx, .. } = envelope;`
   (bench direct_decompose.rs:18, core route_controller.rs:1136,
   route_controller_trait.rs:476/592, direct_tests.rs:286). These three
   core sites are TEMPORARY: task 1.3 replaces the `..` with a real
   claim take. Until task 1.5 attaches claims, all claims are `None`, so
   this is behavior-neutral.
3. No other changes in the forbidden-zone crates: no logic, no imports
   beyond what the field addition requires.

**Tests:**
- `workspace_still_compiles`: `RUSTC_WRAPPER= cargo build --workspace` exits 0; `RUSTC_WRAPPER= cargo test --workspace --lib` exits 0 (pre-existing failures unrelated to this change, if any, must be reported, not fixed).
- `no_claims_attached_yet`: grep the diff for `InFlightClaim::attach` — zero occurrences outside `crates/components/camel-component-api` (attach wiring lands in 1.3/1.5).

**Acceptance:**
- `RUSTC_WRAPPER= cargo build --workspace` exits 0.
- `git diff --stat` shows only the listed files.

- [x] 1.2

### camel-core

#### Task 1.3: context-global counter, install sites, pipeline take-and-hold, inline dispatch claim

**Files:**
- `crates/camel-core/src/context.rs` (modified)
- `crates/camel-core/src/context_builder.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/controller_component_context.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_ext.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/inline_dispatcher.rs` (modified)

**Steps:**
1. `context.rs`: add field `in_flight_total: Arc<AtomicU64>` to
   `CamelContext` and `FromParts` (builder creates the zero counter; thread
   it through `context_builder.rs` `build`). Add
   `pub fn total_in_flight(&self) -> u64 { self.in_flight_total.load(Ordering::Acquire) }`
   with a doc comment: canonical accepted-not-completed count; single
   atomic load = linearizable verdict; distinct from per-route
   `drain_in_flight` stop-bookkeeping (ADR-0043).
2. `context.rs:979` `impl ComponentContext for CamelContext`: override
   `in_flight_counter()` to return `Some(Arc::clone(&self.in_flight_total))`.
3. `controller_component_context.rs`: add field
   `in_flight: Option<Arc<AtomicU64>>` to `ControllerComponentContext`
   (default `None`) and a builder
   `pub(crate) fn with_in_flight(self, counter: Arc<AtomicU64>) -> Self`
   (same style as `ConsumerContext::with_startup`; `pub(crate)` because
   sibling modules `route_compiler_ext.rs` and `route_controller.rs`
   call it), so the existing `new` call sites keep compiling. Override
   `in_flight_counter()` to return `self.in_flight.clone()`.
4. Wire `Some(counter)` into every `ControllerComponentContext`
   construction on the PRODUCER-CREATION path — these are the contexts
   whose `Arc<dyn RuntimeObservability>` reaches `create_producer`
   (route_compiler_ext.rs:196, :340, :745; they also feed
   `endpoint_resolver_factory.rs:46-56` and `step_compilers/mod.rs`
   ~165/~353). The counter must originate from the
   `CamelContext::in_flight_total` created in step 1 — trace the
   compile-state structs that build these contexts (route add/compile
   path starts at `CamelContext`) and thread the `Arc` through. Also
   pass `Some` at `route_controller.rs:647` (unit-of-work path, still a
   production context); `route_controller.rs:473` is `#[allow(dead_code)]`
   test-only and can stay `None`. The e2e test below FAILS if any
   production site is missed — that is its purpose.
5. `route_controller_trait.rs`: at the two `ConsumerContext::new` sites
   (~370 and ~904), chain `.with_in_flight_counter(Arc::clone(&counter))`
   using the same controller-state counter.
6. `route_controller_trait.rs` pipeline drain sites (~476 concurrent path,
   ~592 sequential path) and `route_controller.rs` pre-pipeline path
   (~1136): replace the task-1.2 destructure ellipsis with
   `let ExchangeEnvelope { exchange, reply_tx, in_flight_claim } = envelope;`
   and hold `in_flight_claim` in the task/local scope next to
   `_drain_guard` until the pipeline future completes (concurrent path:
   move it into the spawned task). Drop = release; readiness-failure
   early-returns drop it via scope exit.
7. `inline_dispatcher.rs`: add `in_flight: Option<Arc<AtomicU64>>` to
   `DispatcherState`; at BOTH dispatcher construction sites in
   `route_controller_trait.rs` — route start (~388) AND route resume
   (~939) — pass the counter (a missed resume site would silently leave
   resumed routes' inline dispatch uncounted). In `dispatch`, mint
   `let claim = state.in_flight.as_ref().map(InFlightClaim::attach);` and
   move `claim` into the returned boxed future so it drops when the
   future completes, is dropped (abort), or panics.

**Tests:**
- `total_in_flight_starts_zero`: build a `CamelContext` (existing test builder pattern in context tests) → `total_in_flight() == 0`. `RUSTC_WRAPPER= cargo test -p camel-core total_in_flight --lib`
- `pipeline_residency_counted_until_completion`: start a route (test harness in `route_controller_tests.rs`) whose pipeline parks on a `tokio::sync::Notify`/oneshot barrier; `ConsumerContext::send` an exchange; while the barrier is held assert `total_in_flight() >= 1`; release the barrier, await completion, assert it returns to 0.
- `inline_dispatch_counted`: route with published inline dispatcher (pattern in `inline_dispatcher_tests.rs`); call `dispatch` on a pipeline that parks on a barrier; assert `total_in_flight() >= 1` while parked and 0 after resolution; abort the future mid-park and assert 0 again.
- `readiness_failure_releases_claim`: route whose pipeline service `poll_ready` fails (existing `ready_with_backoff` failure pattern); send an exchange; after the drain site's early return, assert `total_in_flight() == 0`.
- `component_context_exposes_counter`: `CamelContext` (via `Arc`) coerced to `Arc<dyn RuntimeObservability>` → `in_flight_counter()` returns `Some`.
- `seda_enqueue_through_real_context_counted`: e2e wiring smoke — build a `CamelContext` (camel-core already depends on `camel-component-seda`), add a `from: seda:a` route whose pipeline parks on a barrier, produce one exchange to `seda:a` through the real route-compile path; assert `ctx.total_in_flight() >= 1` while parked, and 0 after release + completion.
- `producer_path_receives_counter_spy`: deterministic tripwire for step 4 — register a spy test component whose `create_producer(rt, ..)` records `rt.in_flight_counter().is_some()` keyed by the endpoint's URI base name into a shared `Arc<Mutex<HashMap<String, bool>>>` (per-site flags — one shared bool cannot prove which site executed). A route merely DECLARING an error handler or UoW still compiles its normal `to:` steps at `route_compiler_ext.rs:340` — the :196/:745 sites build producers for the HANDLER/HOOK URIs. So use three distinct spy endpoints, one per site: (a) a plain route with `to: spy:plain` (exercises `route_compiler_ext.rs:340`); (b) a route whose error handling routes failures to `spy:dlc` — set the error-handler/dead-letter target URI (`dlc_uri` / `handled_by`, whichever the DSL names it) to `spy:dlc` (exercises `route_compiler_ext.rs:196`, error-handler producer construction); (c) a route declaring `unit_of_work` with its `on_complete`/`on_failure` hook URI set to `spy:uow`, driven through the route-definition compile path (exercises `route_compiler_ext.rs:745`, UoW producer construction); and (d) if `route_controller.rs:647` remains a wired production UoW-hook path after step 4, a managed-route variant with a `spy:ctl` UoW hook asserting the same. All routes go through the real `CamelContext` compile path. Assert the recorded flag is `true`/`Some` for `spy:plain`, `spy:dlc`, `spy:uow` (and `spy:ctl` if step (d) applies). This test FAILS deterministically if any production `ControllerComponentContext` site from step 4 was missed (the parked-pipeline smoke above cannot catch it — its window is covered by the dispatch claim).

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-core --lib in_flight` and the new test names exit 0.
- `RUSTC_WRAPPER= cargo test -p camel-core --test hexagonal_architecture_boundaries_test` exits 0 (no boundary break).
- `RUSTC_WRAPPER= cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 1.3

#### Task 1.4: claim release paths — abort, panic, queued-drop at stop

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/inline_dispatcher_tests.rs` (modified)

**Steps:**
1. Add a route-level test: send an exchange whose pipeline parks on a
   barrier; stop the route (`stop_route` / context stop) with the barrier
   still held such that the pipeline task is aborted at an await point —
   assert `total_in_flight()` returns to 0 after teardown.
2. Add a queued-drop test: push envelopes carrying claims into a route's
   dispatch channel (via `ConsumerContext::send` with a pipeline that is
   parked so envelopes queue); stop the route so queued envelopes are
   dropped at teardown — assert `total_in_flight() == 0`.
3. Add a panic test: pipeline step that panics inside
   `std::panic::catch_unwind`-compatible harness (use the existing
   panic-in-pipeline test pattern if present; otherwise
   `panic::set_hook` to silence) — assert the claim released and
   `total_in_flight() == 0` after the pipeline task exits.

**Tests:**
- `abort_releases_claim` (step 1): barrier-held pipeline + route stop → `total_in_flight() == 0`. `RUSTC_WRAPPER= cargo test -p camel-core releases_claim --lib`
- `queued_envelope_drop_releases_claim` (step 2): queued envelopes + teardown → `total_in_flight() == 0`.
- `panic_releases_claim` (step 3): panicking pipeline → `total_in_flight() == 0`.

**Acceptance:**
- All three new tests pass; `RUSTC_WRAPPER= cargo test -p camel-core --lib` exits 0.

- [x] 1.4

### camel-component-seda

#### Task 1.5: producer enqueue attach + fanout split + residency/overlap tests

**Files:**
- `crates/components/camel-component-seda/src/lib.rs` (modified)

**Steps:**
1. Add field `in_flight: Option<Arc<AtomicU64>>` to `SedaProducer`
   (struct at ~1015); at the construction site in `create_producer`
   (~659) capture it once via `rt.in_flight_counter()`.
2. Single-mode enqueue (~1077): attach at the same place
   `DepthGuard::count_in` counts in — construct the envelope with
   `in_flight_claim: self.in_flight.as_ref().map(InFlightClaim::attach)`
   before the channel push, so push failure / timeout / queue-full paths
   drop the envelope and release the claim exactly as `DepthGuard` rolls
   back depth.
3. Fanout (~1119-1190): after the permit loop, mint ONE claim (same
   `map(InFlightClaim::attach)`); the first `permit.send(...)` envelope
   carries it; every subsequent envelope carries `claim.split()`. Order
   the loop so each `split()` call happens BEFORE the minted claim is
   moved into the first envelope (e.g. collect the split siblings into a
   `Vec` first, or split at the top of each non-first iteration before
   the move) — otherwise the borrow of the moved-out value will not
   compile. If a panic occurs between minting and sending, the un-sent
   claim drops and releases (mirrors `DepthGuard` rollback).
4. The forwarder (`forward_envelope`, ~993) gets ZERO changes — the
   handoff overlap comes from the envelope shell dropping after
   `ctx.send` returns.
5. Mechanical sweep in this crate's own tests: add `in_flight_claim: None`
   to the in-file test envelope literals at lib.rs ~1465, ~2318, ~2532,
   ~2553, ~2567 (same class of edit task 1.2 does elsewhere).

**Tests:**
- `enqueue_residency_counted`: started seda consumer route + producer with a real counter; enqueue one exchange while the consumer pipeline is parked (existing seda test harness patterns) → counter reads 1 before dequeue; after full pipeline completion → 0. `RUSTC_WRAPPER= cargo test -p camel-component-seda --lib`
- `fanout_splits_claim_per_copy`: fanout endpoint with two subscribers, both pipelines parked → after enqueue, counter reads 2; release and complete one → counter 1; complete the other → 0.
- `handoff_overlap_observed_at_boundary`: deterministic proof that both predecessor and successor claims are live at the seda→route handoff (a sampling loop could miss a transient zero, so do NOT write a poller). Construction: a `from: seda:a` consumer route whose pipeline parks its FIRST exchange on a barrier and whose dispatch channel is small (e.g. capacity 1); enqueue exchange E1 (taken by the parked pipeline) then E2 (fills the channel) then E3 — the forwarder holding E3's shell claim parks INSIDE `ctx.send` with E3's successor claim already attached to the in-send envelope. While parked, assert the counter shows BOTH live handoff claims: exactly (baseline + E1's pipeline-held claim + E2's queued claim + E3's shell claim + E3's in-send successor claim). Then release the barrier and assert eventual return to baseline. This observes the overlap window itself, not a sample of it.
- `no_consumer_rejection_leaves_counter_zero`: enqueue against an endpoint with no active consumers and `discard_if_no_consumers = false` → error returned, counter stays 0.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-component-seda --lib` exits 0.
- `RUSTC_WRAPPER= cargo clippy -p camel-component-seda -- -D warnings` exits 0.

- [x] 1.5

## Phase 2: batch drain on the linearizable snapshot

### camel-cli

#### Task 2.1: batch drain rewrite (poll `total_in_flight()`, delete probe)

**Files:**
- `crates/camel-cli/src/commands/job/batch.rs` (modified)
- `crates/camel-cli/src/commands/job/mod.rs` (modified)

**Steps:**
1. `batch.rs`: delete `DRAIN_ZERO_SAMPLES_REQUIRED`,
   `BatchDepthProbeState`, `BatchDepthProbe`, `BatchProbeLifecycle`, the
   `MetricsCollector` probe impl, and `drain_until_empty`. Rewrite the
   module doc: drain = poll `CamelContext::total_in_flight()` (single
   linearizable load of accepted-not-completed exchanges; covers seda
   queue residency, dispatch, and pipeline residency; raw-sender
   exception per observability spec).
2. Add `pub(crate) async fn drain_until_settled(ctx: &CamelContext, deadline: tokio::time::Instant) -> bool`:
   loop { if `ctx.total_in_flight() == 0` return true; if now >= deadline
   return false; sleep min(100ms, remaining) }. Keep the nap bound and
   deadline discipline of the old `drain_until_empty`.
3. `mod.rs`: delete the probe-construction helper (~1307
   `build_depth_probe`-style fn returning
   `Option<Arc<BatchDepthProbe>>`), the `BatchProbeLifecycle`
   registration (~1422-1423), and the `expected_queues` set construction
   (~1335-1342) — the global counter needs no labels. KEEP the
   fail-closed consumer gate loop above it and all send-target
   validation.
4. Swap the call site (~1193) to `batch::drain_until_settled(&ctx, tokio_deadline)`
   (pass the `CamelContext` handle already in scope).
5. Update the module doc ordering argument in `mod.rs` / `batch.rs` to
   the claim-lifecycle argument from the design doc.

**Tests:**
- `drain_until_settled_waits_for_live_claim`: `CamelContext` + attach a claim to a manually constructed envelope → `drain_until_settled` with a short deadline returns false (deadline hit) while the claim lives; drop the envelope → returns true promptly. `RUSTC_WRAPPER= cargo test -p camel-cli --lib drain`
- `drain_until_settled_zero_completes`: idle context → returns true immediately.
- Existing `batch_typed_arg_coerces_and_drains` (tests.rs:581) still passes end-to-end.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-cli --lib` exits 0 (job module tests included).
- `grep -rn "DRAIN_ZERO_SAMPLES_REQUIRED\|BatchDepthProbe\|BatchProbeLifecycle" crates/camel-cli/src` returns nothing.

- [x] 2.1

#### Task 2.2: job-level drain soundness tests + scheduled-producer gate regression

**Files:**
- `crates/camel-cli/src/commands/job/tests.rs` (modified)
- `crates/camel-cli/src/commands/job/document_tests.rs` (modified)

**Steps:**
1. `tests.rs`: add `batch_self_feed_cannot_false_complete` as an
   IN-PROCESS paused-time test — `#[tokio::test(start_paused = true)]`,
   invoking the job execution entry (the function the binary's clap
   handler dispatches to; it is in-crate and `pub(crate)`-visible)
   directly with a temp fixture dir: route
   `from: seda:a -> delay: 3000 -> to: seda:a`, job `mode: batch`,
   `timeout: 6000`, send target `seda:a`. Under paused time the delay
   and the drain naps are virtual and auto-advanced, so the run is
   deterministic: several drain polls occur while an exchange is parked
   on the virtual delay. Assert the report outcome is `Timeout`
   (exit-2 path) — never `Completed`. This is the paused-time form the
   spec's deterministic-park language sanctions. If the in-process
   entry path proves infeasible (e.g. a process-global signal
   registration conflict that tokio cannot host twice), STOP and report
   `test-design-gap: <detail>` — do NOT silently fall back to a
   wall-clock binary variant.
2. `tests.rs`: add `batch_settled_work_completes` — fixture: two seda
   worker routes recording to `mock:`-style sinks (reuse the
   `batch_typed_arg_coerces_and_drains` fixture shape), short delays;
   assert exit 0 and outcome `Completed`.
3. `document_tests.rs`: extend the consumer-gate suite (~246) with
   `consumer_gate_rejects_timer_and_cron`: `from: timer:1s` and
   `from: cron:0/5 * * * * ?` both rejected at load with the
   fail-closed error (assert the reject message names the scheme).
4. `tests.rs`: add `batch_applies_declared_defaults` — fixture: batch
   job declaring `tier: {default: gold}` with a worker route recording
   `${arg:tier}` to a sink file (reuse the
   `batch_typed_arg_coerces_and_drains` fixture shape), run without
   `--arg tier`; assert exit 0, outcome `Completed`, and the recorded
   value `gold`.

**Tests:**
- `batch_self_feed_cannot_false_complete`: in-process paused-time run of the self-feed fixture → outcome `Timeout` (exit-2 path), never `Completed`. `RUSTC_WRAPPER= cargo test -p camel-cli --lib batch_self_feed`
- `batch_settled_work_completes`: run job binary with settled fixture → exit 0, outcome `Completed`.
- `consumer_gate_rejects_timer_and_cron`: `validate_consumer_uri("timer:1s")` and `validate_consumer_uri("cron:0/5 * * * * ?")` both return `Err` naming the scheme.
- `batch_applies_declared_defaults`: batch run without `--arg tier` → exit 0, `Completed`, recorded value `gold`.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-cli --lib` exits 0 including the four new tests.
- `RUSTC_WRAPPER= cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] 2.2
