# Tasks: direct-inline-fixes

## camel-core (publication guard)

### Task 1.1: Aggregate-split publication guard + capability-absence tests

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs`
  (modified)
- `crates/camel-core/src/lifecycle/adapters/inline_dispatcher_tests.rs`
  (modified)

**Steps:**
1. Add the guard at BOTH dispatcher publication sites in
   `route_controller_trait.rs` — the start path (~:369-378) AND the
   resume path (~:840-871, the block mirroring start's publication):
   when
   `managed.aggregate_split.is_some()`, skip publication entirely (no
   capability entry; producers take the existing capability-unavailable
   channel path). Update the site's comment to record the invariant: an
   aggregate-split route's `managed.pipeline` is an identity shell
   (`compose_pipeline(vec![])`) and must never be exposed to inline
   execution.
2. In `inline_dispatcher_tests.rs`, invert the test at line ~676
   (`aggregate_route_gets_capability`): rename to
   `aggregate_route_never_publishes_capability` and assert
   `ctx.inline_dispatcher().is_none()` on the captured ConsumerContext
   (the test's existing probe-capture mechanism) after start. CHANGE
   THE FIXTURE: the existing `complete_when_size(10)` alone does NOT
   materialize an aggregate split (`find_top_level_aggregate_requiring_split`
   requires a timeout condition or force_completion_on_stop —
   route_helpers.rs:175-186), so the test never exercised
   publication. Use
   `AggregatorConfig::correlate_by("key").complete_when_size(10).force_completion_on_stop(true).build()`
   (mirrors the fixture at route_controller_tests.rs:1410-1413).
3. Add `aggregate_route_resume_never_publishes_capability`: same
   `ctx.inline_dispatcher().is_none()` assertion after suspend→resume of
   the aggregate route (the resume publication site ~:840-871 mirrors
   start; the generic non-aggregate resume test does not cover split
   topology). Fixture note (implementation-verified): the
   force_completion_on_stop fixture cannot suspend (the
   force-completion monitor cancels the pipeline plane on consumer
   exit → Stopped, and resume rejects non-Suspended — route_controller
   .rs:1232-1240); the resume test therefore uses the
   `complete_on_timeout(Duration::from_secs(600))` split fixture
   (has_timeout_condition → split materializes; canonical pattern per
   route_helpers.rs:333).
4. Keep the non-aggregate Sequential control assertion in the same file
   green (existing tests own it).

**Tests:** (`cargo test -p camel-core --lib -- lifecycle::adapters::inline_dispatcher`
from the worktree root)
- `aggregate_route_never_publishes_capability`: split route started →
  registry entry exists, `dispatcher` is `None`.
- `aggregate_route_resume_never_publishes_capability`: split route
  started → suspended → resumed → `dispatcher` still `None`.
- `expected`: both FAIL at current HEAD (the capability IS published for
  split routes today — that is rc-2sba), PASS after step 1.

**Acceptance:**
- `cargo test -p camel-core --lib -- lifecycle::adapters::inline_dispatcher`
  exits 0 (all existing tests in the file stay green).
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: End-to-end direct-entry aggregate regression (N→1)

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs`
  (modified)

**Steps:**
1. Add `direct_entry_aggregate_delivers_single_aggregated_reply`,
   mirroring the fixture pattern of
   `aggregate_force_completion_on_stop_emits_pending_bucket_without_timeout`
   (line ~1397: mock component registry, DefaultRouteController,
   BuilderStep::Aggregate with AggregatorConfig) with the entry
   swapped from `timer:` to `direct:agg-in` and the completion policy
   `AggregatorConfig::correlate_by("key")
   .complete_on_size_or_timeout(5, Duration::from_secs(2)).build()`
   (camel-api aggregator.rs:307 — the Timeout arm materializes the
   split; natural completion at 5 delivers without waiting the 2s
   ceiling). Add a driver route
   `from("direct:driver").to("direct:agg-in")`, then send N=5 exchanges
   to `direct:driver`, each with a distinct body and the same `key`
   header, so all fragments enter one correlation bucket.
2. Put `mock:sink` after the aggregate step and assert it receives
   exactly ONE completed aggregate carrying all 5 fragment bodies (the
   mirrored tests' bucket-content assertion helpers apply). Pending
   replies for the first four inputs are not completed aggregates and
   must not reach this post-aggregate sink.
3. Run BEFORE the Task 1.1 guard lands to record the red proof of
  rc-2sba (fragments returned unprocessed → zero aggregated replies);
  after the guard, the test must pass. If Task 1.1 already landed,
  temporarily reverting the guard locally reproduces the red state
  (do not commit the temporary revert).

**Tests:**
- `direct_entry_aggregate_delivers_single_aggregated_reply`: 5 fragments
  via `to("direct:agg-in")` → exactly 1 reply containing all 5 bodies.
- `expected`: FAIL at current HEAD (rc-2sba: identity pipeline returns
  each fragment unprocessed, no aggregation); PASS after Task 1.1.

**Acceptance:**
- `cargo test -p camel-core --lib -- direct_entry_aggregate` exits 0.
- No existing aggregate test modified.

- [x] 1.2

## camel-direct (b′ visibility) + camel-test (contract tests)

### Task 1.3: Route unhandled dispatch failures through the b′ emission

**Files:**
- `crates/components/camel-direct/src/lib.rs` (modified)
- `crates/camel-test/tests/metrics_wiring_test.rs` (modified — new
  cases only; the red `late_registration_after_routes_observed` case
  is the contract and must NOT be edited)

**Steps:**
1. Restructure the producer call (`lib.rs:455-540`): the initial
   registry lookup failure currently `?`-exits at ~:483 BEFORE the b′
   emission block at :518-532. Route the lookup error into the same
   `result` flow so the emission block (context-threaded
   `self.runtime` handle) fires exactly once for it. ATTRIBUTION for
   the no-entry case: emit with an endpoint-derived id —
   `increment_errors(&format!("direct:{name}"),
   "b-prime:direct:send-and-wait")` — which distinguishes the
   component signal from the producing route's traced wrapper (that
   records under the producing route id); the no-double-count test
   filters the collector by this attribution.
2. Timeout branch: the outer `.map_err(|_| dispatch_timeout_error(&name))`
   at ~:538 bypasses the inner emission block entirely (tokio drops
   the inner future on expiry) — a timeout failure emits NOTHING today.
   Emit b′ on the timeout branch before returning the error (same
   handle; endpoint-derived attribution; no ConsumerStopping ambiguity
   exists on this freshly-constructed error).
3. Verify `ConsumerStopping` failures do NOT emit on the inner `result`
   path: the variant is `CamelError::ConsumerStopping` (produced at
   `inline_dispatcher.rs:156`; the channel path can never produce it);
   add the `matches!` exclusion at the emission site, keeping the
   timeout/pipeline/lookup paths emitting.
4. Run the acceptance test
   `cargo test -p camel-test --test metrics_wiring_test --
   late_registration_after_routes_observed` (currently red on main)
   — it MUST turn green with the producer-side fix. Wiring verified:
   the runtime handle is `Arc::clone(&component_ctx)` taken at route
   compile (`route_compiler_ext.rs:347-348`), and the test registers
   its RecordingLifecycle BEFORE `start()` compiles routes, so the
   producer's `self.runtime.metrics()` reaches the collector. The
   emission owner is PINNED producer-side (single site, one handle) —
   no dispatcher-side fallback exists in this change. camel-core
   files affected by Fix B: NONE (all changes live in camel-direct
   lib.rs + camel-test tests).
5. Add the contract cases to `metrics_wiring_test.rs` (mirror the
   existing `add_failing_route`/`RecordingCollector` harness): named
   below. Exactly-once cases drive SINGLE-SHOT
   (`drive_exchange(ctx, Duration::ZERO)` semantics after
   `wait_for_started`) — the default `run_one_failing_exchange` retries
   every Err for 1s, which would yield dozens of emissions and make
   exactly-once unassertable.

**Tests:** (`cargo test -p camel-test --test metrics_wiring_test`)
- `late_registration_after_routes_observed` — existing, RED on main →
  GREEN (acceptance; do not edit).
- `direct_lookup_failure_emits_b_prime_once`: route
  `from("direct:entry2").to("direct:missing2?failIfNoConsumers=false")`,
  recording lifecycle registered before start, one exchange run →
  collector saw exactly one `increment_errors:` call.
- `direct_pipeline_error_emits_b_prime_once`: consumer route
  `from("direct:boom").process(|_| Err(CamelError::ProcessorError("bench boom".into())))`
  (unhandled; variant pattern from route_controller_tests.rs:2717),
  producer `to("direct:boom")` → exactly one `increment_errors:`.
- `direct_timeout_error_emits_b_prime_once`: inline-eligible consumer
  whose pipeline sleeps 50ms; producing route
  `to("direct:slow?timeout_ms=1")` (the param `DirectConfig::from_uri`
  reads at lib.rs:146-151; unknown camelCase params are silently
  ignored — use the snake_case name) → the single timed section
  (lookup+admission+execution) expires → exactly one
  `increment_errors:`.
- `direct_consumer_stopping_no_emit`: dispatch failing with the
  consumer-stop surrender — mirror the stop-race shape from
  camel-core `inline_dispatcher_tests.rs:717` (inline dispatch parked,
  target consumer stopped mid-flight, `CamelError::ConsumerStopping`
  surfaces) → zero `increment_errors:` calls attributable to the
  surrender.
- `direct_wired_route_no_double_count`: producing route WITH pipeline
  tracing wired dispatching to
  `direct:missing3?failIfNoConsumers=false` → the component
  signal appears exactly once in the collector (traced-wrapper
  pipeline telemetry is separate and additive; assert the component
  count, not the sum).
- `expected` (per case, at current HEAD):
  `direct_lookup_failure_emits_b_prime_once` RED (lookup `?`-exits
  before the emission block); `direct_timeout_error_emits_b_prime_once`
  RED (outer timeout mapping bypasses the emission block);
  `direct_consumer_stopping_no_emit` RED (ConsumerStopping flows
  through `result` today and DOES emit);
  `direct_wired_route_no_double_count` RED (its dispatch is the lookup
  case — no component emission today);
  `direct_pipeline_error_emits_b_prime_once` PASSES at current HEAD
  (the result branch already emits) — it is a REGRESSION GUARD for
  the restructuring, not a red-first case. All PASS after the fix.

**Acceptance:**
- `cargo test -p camel-test --test metrics_wiring_test` exits 0
  (whole file).
- `cargo test -p camel-component-direct` exits 0 (existing channel-path
  emission tests stay green unmodified).
- `cargo clippy -p camel-direct -p camel-test --all-targets -- -D
  warnings` exits 0.

- [x] 1.3

## verification

### Task 1.4: Full-suite verification + bench gate

**Files:**
- `openspec/changes/direct-inline-fixes/tasks.md` (modified — checkbox
  closeout only)

**Steps:**
1. Run from the worktree: `cargo test -p camel-core --lib`, `cargo
   test -p camel-direct`, `cargo test -p camel-test --test
   metrics_wiring_test`, `cargo test -p camel-core --lib -- lifecycle`
   — all green.
2. Full test surface: `cargo test -p camel-test` (the whole
   integration crate, not only metrics_wiring).
3. Lint gates: `cargo fmt --check --all`,
   `cargo clippy --workspace --all-features --exclude camel-cli
   --exclude camel-component-kafka --exclude security-keycloak
   --exclude security-wasm-policy -- -D warnings`,
   `cargo clippy -p camel-component-kafka --all-targets -- -D
   warnings`, `cargo clippy -p camel-cli -- -D warnings`,
   `cargo xtask lint-unwrap`, `cargo xtask lint-secrets`,
   `cargo xtask lint-non-exhaustive`, `cargo xtask lint-log-levels`,
   `cargo xtask lint-ignore`, `cargo xtask lint-metric-labels`,
   `cargo xtask lint-publish-cycles`, `cargo xtask
   lint-component-deps`, `cargo xtask lint-gate-forwarding`,
   `cargo xtask lint-context-citations`, `cargo xtask schema
   --check`, and the changelog gate in its local form
   `cargo xtask changelog --check --from main --to HEAD` (run from
   the feature branch — same lint as CI's lint-commits minus the
   `git fetch`, which the conductor never performs).
4. Bench gate (no fallback leakage): `cargo bench -p camel-bench
   --bench direct` — the GATE is the exit code carrying the built-in
   `assert_inline_dispatch` (benches/direct.rs:110-143 fails the run
   if a dispatch took the channel path); the plain-hop median
   (~1185 ns ± jitter) is informational.
5. Record results in the task report; tick checkboxes.

**Tests:**
- Verification harness task — the asserts are the commands and exit
  codes above.
- `expected`: all pass after Tasks 1.1-1.3.

**Acceptance:**
- Every command in steps 1-4 exits 0 (bench gate = assert_inline_dispatch
  exit status; median informational).
- All four task checkboxes ticked.

- [x] 1.4
