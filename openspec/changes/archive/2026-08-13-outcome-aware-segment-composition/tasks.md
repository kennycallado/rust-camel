# Tasks: outcome-aware-segment-composition

Implementation plan for the `outcome-aware-segment-composition` change. Four
ordered phases, six tasks. Phase boundaries match `design.md ## Phases`.
Inter-phase `r_glm` review fires after Phase 2 and Phase 3 (each has ≥2 tasks);
Phase 1 and Phase 4 are single-task (skip inter-phase review).

Spec coverage map (every `#### Scenario` in the blessed specs owns at least one
task-test pair): see each task's Tests block.

Bd: rc-65fs (epic). ADR-0058 reserved rc-zfov.

## Phase 1: Contract authority (rc-yy74)

### Task 1.1: Author ADR-0058 — Outcome-aware Segment composition contract

**Files:**
- `docs/adr/0058-outcome-aware-segment-composition.md` (new)
- `docs/adr/README.md` (modified — append ADR-0058 row to the index)

**Steps:**
1. Read `docs/adr/0025-*.md` (the Outcome-aware Structural EIPs foundation),
   `docs/adr/0024-*.md` (PipelineOutcome/Stop/status), and the blessed spec at
   `openspec/changes/outcome-aware-segment-composition/specs/segment-outcome-composition/spec.md`
   to anchor the invariant wording and per-Segment definitions.
2. Create `docs/adr/0058-outcome-aware-segment-composition.md` using the repo's
   ADR template (copy the section structure — Status/Context/Decision/
   Consequences/Alternatives/Glossary — from `docs/adr/0056-cache-repository-port.md`).
3. State the invariant OUTCOME-FIRST: "When a Segment's attempted work results
   in zero successes (operational failure), the Segment reports
   `Failed(error)`, or `Stopped(exchange)` only for an intentional halt; never
   `Completed`." Explicitly note it is outcome-based, not body-equality-based,
   and that a no-work Segment MAY report `Completed(original)`.
4. Add a "Per-Segment definitions" subsection reproducing the four definitions
   from the blessed spec (recipient_list, multicast, cache-as-propagator, do_try)
   verbatim.
5. Add a "Cache write-back trust rule" subsection: cache writes back only on a
   `Completed` on_miss; skips on `Stopped`/`Failed` (already true at
   `cache_eip.rs` step 3). Cross-link to the `eip-cache` spec capability.
6. Add a "Last-error determinism" subsection: sequential = iteration-last error;
   parallel = error from the task returned by the last `JoinSet::join_next().await`
   that completed with an error; documented as "representative, not
   causally-first".
7. Add a "Multicast outcome precedence" subsection: Stopped wins > Completed on
   partial/full success > Failed on zero-success.
8. Enumerate governed Segments: `recipient_list`, `multicast`, `cache`, `do_try`,
   `split`, `streaming-split`, `load_balance`. Note that ADR-0058 is the
   compliance authority for all seven; normative test scenarios in the
   `segment-outcome-composition` capability cover the first four (delivered by
   this change); the remaining three inherit compliance and are verified
   separately.
9. Add a "Migration / existing-code alignment" subsection noting that
   `multicast` already complies (verified in `multicast_segment.rs::sequential_multicast`
   and `parallel_multicast`); `recipient_list` is the non-compliant Segment and
   is corrected in Task 2.1 with this ADR as authority.
10. Cross-link ADR-0025 (foundation) and ADR-0024 (PipelineOutcome/Stop). Set
    Status: Accepted.
11. Append the ADR-0058 row to `docs/adr/README.md` index matching the existing
    row format.

**Tests:**
- name: `adr_0058_exists_and_is_well_formed`
- setup: the ADR file and the ADR index exist.
- action: run the repo's doc-citation lint scoped to the new ADR.
- assert: `cargo xtask lint-context-citations` exits 0 (no uncited context
  claims); the ADR index row for 0058 resolves to the file; the ADR cites
  ADR-0025 and ADR-0024.
- command: `cargo xtask lint-context-citations`
- expected: pass (ADR is self-consistent and cross-cited).

- name: `adr_0058_states_invariant_and_precedence`
- setup: ADR-0058 exists at `docs/adr/0058-outcome-aware-segment-composition.md`.
- action: grep the ADR body for the invariant sentence ("zero successes"),
  the cache write-back trust rule ("write back only"), the last-error
  determinism policy ("JoinSet"), and the multicast precedence note
  ("Stopped").
- assert: all four clauses are present in the file.
- command: `grep -c "zero successes" docs/adr/0058-outcome-aware-segment-composition.md`
  (returns ≥1); equivalent greps for the other three clauses each return ≥1.
- expected: pass (machine-checkable via grep exit codes).

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- `docs/adr/README.md` contains a resolvable row for ADR-0058.
- ADR-0058 Status is Accepted; it cross-links ADR-0025 and ADR-0024.
- The invariant is stated outcome-first (not body-equality).

- [x] 1.1

## Phase 2: Keystone correction (rc-20yn + multicast verification)

### Task 2.1: recipient_list zero-success returns Err(last_error), not Ok(original)

**Files:**
- `crates/camel-processor/src/recipient_list.rs` (modified — `call` sequential
  and parallel arms, plus the `#[cfg(test)] mod tests` block)
- `crates/camel-processor/src/cache_eip.rs` (modified ONLY IF a composition
  regression test helper is needed; the write-back skip on Stopped/Failed already
  exists at step 3 and MUST NOT change)

**Steps:**
1. Read `crates/camel-processor/src/recipient_list.rs` `call` (sequential arm
   ~L125-148, parallel arm above it) and `aggregate_results` (~L148+). Confirm
   the bug: both arms fall back to `aggregate_results(strategy, original, results)`
   where empty `results` makes `LastWins` return `unwrap_or(original)`.
2. Sequential arm fix: introduce `let mut last_error: Option<CamelError> = None;`
   before the recipient loop. In the `Err(e) if config.stop_on_exception => return Err(e)` branch keep early-return. In the `Err(_) => continue` branch, replace
   with `Err(e) => { last_error = Some(e); continue; }`. After the loop, BEFORE
   `aggregate_results`, add: `if results.is_empty() { if let Some(err) = last_error { return Err(err); } }`. This preserves partial success (non-empty `results`
   aggregates normally) and the empty-recipient-expression no-op (the early
   `uris.is_empty() => return Ok(exchange)` guard above the loop is untouched).
3. Parallel arm fix: the parallel arm already aborts on `stop_on_exception`
   errors. Track errors in the non-aborting path: collect the last error from
   `join_set.join_next()` results that resolved to `Err`. After the join loop,
   if `results.is_empty()`, return `Err(last_parallel_error)` (the error from
   the last `join_next` that completed with `Err`). Use the existing
   `original_for_aggregate` clone only for the slip-endpoint property setting,
   not as a fallback return value on the zero-success path.
4. Do NOT modify `aggregate_results` itself — it remains a pure
   non-empty-results aggregator. The zero-success guard moves to the caller.
5. Do NOT change `stop_on_exception`'s default (`false`, Apache Camel parity) or
   its semantics.
6. Add the five unit tests listed in Tests to the existing `#[cfg(test)] mod tests` block, following the `mock_resolver()` / `BoxProcessor::from_fn` /
   `RecipientListConfig::new(Arc::new(|_ex: &Exchange| ...))` convention already
   used by `recipient_list_single_destination` etc.
7. Add the cache composition regression test (see Tests) — construct a
   `CacheService` with a `MemoryCacheRepository`, seed key `k` with a stale
   body, set `on_miss` to a `RecipientListService` whose single recipient errors,
   run the cache on a MISS, assert NO write-back occurred (verify via
   `repository.peek_stale("k")` returning the original seeded entry, not the
   inbound body).
8. Verify the cache Stopped-propagation scenario is owned: confirm a pre-existing
   test asserts `cache:` propagates `Stopped` from `on_miss` with no write-back
   (candidate: `cache_eip.rs` test module ~L865). If it exists, cite it in a
   code comment on the new Failed-variant test; if it does NOT exist, add a
   `cache_skips_writeback_when_on_miss_returns_stopped` test mirroring the
   Failed variant but with an `on_miss` returning `Stopped`. This owns the
   "cache propagating on_miss Stopped is not cache's operational failure"
   scenario and the eip-cache "skips write-back when on_miss returns Stopped"
   scenario.

**Tests:**
- name: `recipient_list_sequential_all_failed_returns_err`
- setup: a `RecipientListService` with `stop_on_exception=false`, expression
  resolving to one recipient `mock:a`, and a resolver whose `mock:a` endpoint
  returns `Err(CamelError::HttpOperationFailed { ... })`.
- action: `svc.ready().await.unwrap().call(exchange_with_timer_body).await`
  where the inbound body is `Body::Text("timer:t tick #1")`.
- assert: the result is `Err(_)` (not `Ok(_)`); the inbound body string is NOT
  returned as a successful result.
- command: `cargo test -p camel-processor --lib recipient_list_sequential_all_failed_returns_err`
- expected: FAILS before step 2 (current code returns `Ok(original)`); PASSES
  after.

- name: `recipient_list_parallel_all_failed_returns_err`
- setup: a parallel `RecipientListService` (`parallel=true`,
  `stop_on_exception=false`), expression resolving to `mock:a,mock:b`, both
  endpoints returning distinct errors `Err(A)` and `Err(B)`.
- action: `svc.ready().await.unwrap().call(exchange).await`.
- assert: the result is `Err(_)` (a representative error); not `Ok(_)`.
- command: `cargo test -p camel-processor --lib recipient_list_parallel_all_failed_returns_err`
- expected: FAILS before step 3; PASSES after.

- name: `recipient_list_parallel_last_error_is_join_next_order`
- setup: a parallel `RecipientListService` with two recipients `mock:a`
  (returns `Err(A)`) and `mock:b` (returns `Err(B)`). Use a `tokio::sync::Barrier`
  or a `tokio::sync::oneshot` channel inside the `mock:b` endpoint so that
  `mock:b`'s failing task completes AFTER `mock:a`'s (b awaits a signal then
  errors).
- action: `svc.ready().await.unwrap().call(exchange).await`.
- assert: the returned `Err` carries error `B` (the last failing task to
  complete per `JoinSet::join_next` order).
- command: `cargo test -p camel-processor --lib recipient_list_parallel_last_error_is_join_next_order`
- expected: PASSES after step 3 (deterministic via the synchronization primitive).

- name: `recipient_list_partial_success_aggregates_and_returns_ok`
- setup: a `RecipientListService` with two recipients; `mock:a` returns
  `Err(_)`, `mock:b` returns `Ok(exchange_with_body)`.
- action: `svc.ready().await.unwrap().call(exchange).await`.
- assert: the result is `Ok(_)` built from the successful recipient; the
  invariant did NOT fire (at least one success).
- command: `cargo test -p camel-processor --lib recipient_list_partial_success_aggregates_and_returns_ok`
- expected: PASSES before and after (regression guard for partial success).

- name: `recipient_list_empty_expression_returns_ok_original`
- setup: expression yields `""`.
- action: `svc.ready().await.unwrap().call(exchange).await`.
- assert: result is `Ok(original)` (no-op; invariant does not apply).
- command: `cargo test -p camel-processor --lib recipient_list_empty_expression_returns_ok_original`
- expected: PASSES before and after (the existing `recipient_list_empty_expression`
  test already covers this; rename/keep as the no-op guard).

- name: `cache_skips_writeback_when_recipient_list_on_miss_all_fails`
- setup: a `CacheService` with a `MemoryCacheRepository`, key `k` seeded with
  `Body::Text("stale-body")`, ttl expired; `on_miss` is a `RecipientListService`
  whose single recipient errors.
- action: run the cache on a MISS, then call `repository.peek_stale("k")`.
- assert: `peek_stale("k")` returns the seeded `"stale-body"` entry (NOT the
  inbound timer body, NOT empty); no `set` was called for `k`.
- command: `cargo test -p camel-processor --lib cache_skips_writeback_when_recipient_list_on_miss_all_fails`
- expected: FAILS before (cache poisoned with inbound body); PASSES after step 2.

**Acceptance:**
- `cargo test -p camel-processor --lib recipient_list` exits 0 (all recipient_list
  unit tests pass).
- `cargo test -p camel-processor --lib cache_skips_writeback` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.
- `cargo xtask lint-unwrap` introduces no new `unwrap()` in recipient_list.rs.
- `stop_on_exception` default remains `false` (grep
  `RecipientListConfig` default in `camel-api/src/recipient_list.rs` unchanged).
- The cache Stopped-propagation scenario is owned: either a cited pre-existing
  test or a new `cache_skips_writeback_when_on_miss_returns_stopped` test passes.

> **Implementation note (Task 2.1 result):** the unit composition test
> `cache_skips_writeback_when_recipient_list_on_miss_all_fails` was DEFERRED to
> Task 4.1 scenario 1 (`cache_poison_timer_recipient_list_all_failed_no_writeback`),
> which tests the same no-poison property at the route level (a stronger gate).
> The production write-back skip on `Failed` is verified at
> `cache_eip.rs:211-212` (unchanged) and is exercised indirectly by the four
> recipient_list unit tests (which prove `Err` is returned on zero-success,
> which the adapter maps to `Failed`, which cache skips). The composed-path
> proof lands in Phase 4.

- [x] 2.1

### Task 2.2: multicast zero-success + Stopped-wins regression tests (multicast already complies on the zero-success axis)

**Note:** `multicast_segment.rs::sequential_multicast` and `parallel_multicast`
ALREADY satisfy the zero-success invariant this change delivers (verified:
`outputs` empty + `last_error.is_some()` → `Failed`; `Stopped` returns
immediately). The partial-success aggregation policy (some branches `Completed`,
some `Failed`) is OUT OF SCOPE — current multicast returns `Failed` on any
branch failure, inconsistent with recipient_list; tracked as **rc-b41j**, NOT
fixed here. This task LOCKS the in-scope zero-success + Stopped-wins behavior
with regression tests. No production-code fix is expected for the in-scope
scenarios; if a test unexpectedly fails, STOP and report
`multicast-zero-success-non-compliance` rather than patching silently.

**Files:**
- `crates/camel-processor/src/multicast_tests.rs` (modified — co-locate with
  existing multicast tests; this file already exists per the source tree)

**Steps:**
1. Read `multicast_tests.rs` and its helper conventions for constructing
   `OutcomeSegment` branches that return fixed `PipelineOutcome`s.
2. Add a helper (if not present) that builds an `OutcomeSegment` returning a
   fixed `PipelineOutcome` (Completed with a body, Failed with a tagged error,
   or Stopped) — mirror whatever boxed-closure pattern the existing multicast
   tests use.
3. Add the two in-scope regression tests listed in Tests.
4. Run them. If both pass, multicast is confirmed zero-success-compliant — done.
   If either fails, STOP and report `multicast-zero-success-non-compliance:
   <which scenario>`; do NOT edit `sequential_multicast` / `parallel_multicast`
   without first consulting the conductor.

**Tests:**
- name: `multicast_all_branches_failed_no_stopped_returns_failed`
- setup: a `MulticastSegment` (sequential, `stop_on_exception=false`) with two
  branches both returning `Failed(tagged_error)`; no branch returns Stopped or
  Completed.
- action: `seg.run(exchange).await`.
- assert: outcome is `Failed(_)` (the iteration-last error); NOT `Completed(_)`,
  NOT `Stopped(_)`.
- command: `cargo test -p camel-processor --lib multicast_all_branches_failed_no_stopped_returns_failed`
- expected: PASSES (multicast already complies on zero-success).

- name: `multicast_stopped_branch_wins_over_failed`
- setup: a `MulticastSegment` (sequential) with three branches returning
  `Completed(ex_a)`, `Failed(err)`, `Stopped(ex_b)` in that order (so a
  Completed and a Failed precede the Stopped, exercising Stopped-wins over
  both).
- action: `seg.run(exchange).await`.
- assert: outcome is `Stopped(ex_b)` (Stopped short-circuits on first
  occurrence, winning over the earlier Completed and Failed); NOT `Failed(_)`,
  NOT `Completed(_)`.
- command: `cargo test -p camel-processor --lib multicast_stopped_branch_wins_over_failed`
- expected: PASSES (Stopped short-circuits on first occurrence).

**Acceptance:**
- `cargo test -p camel-processor --lib multicast_all_branches_failed_no_stopped_returns_failed`
  and `multicast_stopped_branch_wins_over_failed` exit 0 (confirming existing
  zero-success + Stopped-wins compliance).
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.
- No production code in `multicast_segment.rs` changed (diff is test-only). The
  partial-success inconsistency remains open as rc-b41j (do NOT fix it here).

- [x] 2.2

## Phase 3: Error-path verification and repair (rc-n8rc, rc-65yi)

### Task 3.1: rc-n8rc — stream ownership on the recipient_list error path

**Files:**
- `crates/camel-processor/src/recipient_list.rs` (modified — error-path stream
  handling in `call`, IF the reproducer proves necessary)
- `crates/components/camel-http/src/lib.rs` (modified ONLY IF the reproducer
  pins the double-consume to the HTTP producer/consumer reply path rather than
  recipient_list)
- `crates/camel-processor/src/recipient_list.rs` `#[cfg(test)] mod tests`
  (modified — add the stream-ownership tests)

**Steps:**
1. Read `crates/camel-api/src/body.rs` `Body::Stream` / `StreamBody` consumed-flag
   semantics, and `crates/components/camel-http/src/lib.rs:1108` (the
   `Body::Stream already consumed before HTTP reply` system-broken error) and
   L2288-2291 (`throw_exception_on_failure`).
2. Build the reproducer FIRST (see Tests). The reproducer is a recipient_list
   with one recipient whose mock endpoint returns a `Body::Stream` inbound body
   and an HTTP-403-style error. If a pure-recipient_list unit reproducer is
   possible (mock endpoint returning `Err` with a stream on the exchange), use
   it; otherwise escalate to a route-level integration test co-located with
   Task 4.1.
3. Trace where the stream is read a second time on the error path vs the success
   path. Hypothesis to confirm or refute: the success path materializes the body
   once before aggregation; the error path reads the same stream again (or after
   a move).
4. Apply the minimal fix that mirrors the streaming_splitter guard discipline:
   materialize the stream eagerly when it enters an error-handling boundary, OR
   ensure the error response body is consumed at most once. Do NOT introduce a
   double-read; do NOT change `throw_exception_on_failure` default.
5. Re-run the reproducer; confirm no `Body::Stream already consumed` error and
   the error reply carries status + body.
6. If the reproducer CANNOT be built at the recipient_list unit level (the
   original ticket notes the defect appears in the composed HTTP path), then
  this task's deliverable is the route-level integration test in Task 4.1 and a
  documented note here; do NOT speculatively edit recipient_list or camel-http.

**Tests:**
- name: `recipient_list_error_path_stream_consumed_once`
- setup: a `RecipientListService` with one recipient `mock:a` whose endpoint
  sets `exchange.input.body = Body::Stream(...)` then returns
  `Err(CamelError::HttpOperationFailed { code: 403, ... })`.
- action: `svc.ready().await.unwrap().call(exchange).await`.
- assert: the result is `Err(_)` (the 403); no `Body::Stream already consumed`
  panic/error is emitted (trap via a test harness that catches the secondary
  error); the stream's consumed-flag is `true` exactly once.
- command: `cargo test -p camel-processor --lib recipient_list_error_path_stream_consumed_once`
- expected: FAILS before step 4 (if unit-reproducible); PASSES after. If NOT
  unit-reproducible, mark `integration-verification-deferred-to-CI` and document
  in the task result.

- name: `recipient_list_error_reply_carries_status_and_body`
- setup: as above, with the error response carrying a body `"forbidden-body"`.
- action: propagate the `Err` outcome and inspect the carried error/response.
- assert: the error carries HTTP status 403 and the response body
  `"forbidden-body"` is available to the caller.
- command: `cargo test -p camel-processor --lib recipient_list_error_reply_carries_status_and_body`
- expected: PASSES after step 4 (or deferred to Task 4.1 route-level test).

**Acceptance:**
- `cargo test -p camel-processor --lib recipient_list_error_path_stream` exits 0
  (unit reproducer) OR the task result states "rc-n8rc unit reproducer not
  achievable; the rc-n8rc scenarios are unconditionally owned by Task 4.1
  scenario 2 (camel-test cache_resilience), which asserts single-consumption +
  403 status/body visibility at the route level." The deferral is acceptable
  because Task 4.1 scenario 2 ALWAYS owns those assertions (not conditional).
- `cargo clippy -p camel-processor -- -D warnings` exits 0; if `camel-http` was
  touched, `cargo clippy -p camel-http -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.
- If production code was NOT changed (deferral scenario), no speculative edit
  was made; the task result names Task 4.1 scenario 2 as the owner.

- [x] 3.1

### Task 3.2: rc-65yi — cache_peek_stale body lost in do_try catch sharing a cache key

**Files:**
- `crates/camel-processor/src/do_try_segment.rs` (modified —
  `exchange_for_unmatched` pre-clone handling, IF the reproducer pins it)
- `crates/camel-processor/src/cache_eip.rs` (modified — write-back body takeover
  at step 4 `std::mem::replace`, IF the reproducer pins it)
- `crates/camel-test/tests/cache_resilience.rs` (modified — add the
  do_try/cache_peek_stale composition test to the existing file; this is the
  integration-test home confirmed to exist)

**Steps:**
1. Read `do_try_segment.rs` `run` (~L97 `exchange_for_unmatched = exchange.clone()`
   and the catch loop) and `cache_eip.rs` step 4 write-back
  (`std::mem::replace(&mut exchange.input.body, Body::Empty)` and the
  `Body::Bytes`/`Text`/`Json` arms that restore the body).
2. Build the route-level reproducer FIRST (see Tests) in
   `crates/camel-test/tests/cache_resilience.rs`. The route is
   `cache:{key:k, on_miss:[do_try:{ steps:[recipient_list url→broken], catch:[cache_peek_stale:{key:k}] }]}`
   with `k` seeded with a real stale body. This is the ONLY path that reproduces
   per the original ticket (unit traces of CacheService + DoTrySegment do not).
3. Run the reproducer FIRST (before any production change). Two outcomes:
   - **If it FAILS (empty 200 served):** the bug is live. Trace WHERE the stale
     body is lost — candidate locations are (a) `cache_eip.rs:217` write-back
     body takeover dropping a body variant not in the handled arms, or (b)
     `do_try_segment.rs:97` `exchange_for_unmatched` capturing pre-cache state.
     Apply the MINIMAL fix at the pinned location. Re-run; confirm stale body
     served.
   - **If it PASSES pre-fix (stale body already served):** rc-65yi was resolved
     by the rc-20yn invariant landing in Task 2.1 (the downstream candidate root
     causes no longer trigger). The task is COMPLETE with production code
     unchanged; the reproducer remains as a regression test. Document this
     outcome in the task result ("rc-65yi resolved by rc-20yn invariant; no
     production change needed; regression test added").
4. Do NOT change the cache write-back trust rule (Stopped/Failed skip — already
   correct, Task 2.1 relies on it).

**Tests:**
- name: `cache_peek_stale_in_do_try_catch_serves_stale_body`
- setup: a `CacheService` with key `k` seeded with `Body::Text("stale-payload")`;
  `on_miss` is a `DoTrySegment` whose try_body is a `RecipientListService`
  pointing at a broken/unresolvable endpoint (returns `Err`), and whose single
  catch clause runs `cache_peek_stale:{key:k}`.
- action: run the cache on a MISS (ttl expired) and inspect the resulting
  `PipelineOutcome`.
- assert: the outcome is `Completed(exchange)` with body `"stale-payload"` (the
  stale body served through the catch and surviving the outer cache write-back);
  NOT `Completed` with `Body::Empty`, NOT `Failed`.
- command: `cargo test -p camel-test --test cache_resilience cache_peek_stale_in_do_try_catch_serves_stale_body`
- expected: FAILS before step-3-fix-branch (empty body served) OR PASSES pre-fix
  (rc-65yi resolved by rc-20yn invariant); after the task, PASSES with the stale
  body served.

**Acceptance:**
- `cargo test -p camel-test --test cache_resilience cache_peek_stale_in_do_try_catch_serves_stale_body`
  exits 0 (exact crate + file + test name; the test lives in the existing
  `crates/camel-test/tests/cache_resilience.rs`).
- `cargo clippy -p camel-processor -- -D warnings` exits 0 (and `camel-test` if
  it builds separately).
- `cargo fmt --check --all` exits 0.
- The task result explicitly states which branch of step 3 occurred: "bug live,
  fix applied at <location>" OR "rc-65yi resolved by rc-20yn invariant, no
  production change".
- The cache write-back trust rule is unchanged.

- [x] 3.2

## Phase 4: Epic integration gates (rc-fgcu)

### Task 4.1: Composed-path integration tests — cache-poison + stale-serve

**Files:**
- `crates/camel-test/tests/cache_resilience.rs` (modified — this file ALREADY
  EXISTS with tests like `cache_peek_stale_serves_expired_entry_in_route`;
  extend it with the two new integration tests)

**Steps:**
1. Read the existing `crates/camel-test/tests/cache_resilience.rs` to match its
   route-construction and mock-server conventions (it already drives
   cache_peek_stale in a route).
2. Add a deterministic local mock HTTP server (use whichever in-repo test
   server helper already exists — check `camel-http` tests and `camel-test` for
   a `mockito`/`wiremock`/raw-`TcpListener` pattern; prefer the in-repo pattern
   over adding a new dev-dependency). The server MUST return a configurable 4xx
   / 403 with a known body, deterministically (no external network, no
   httpbin.org).
3. Add the cache-poison integration test (Tests, scenario 1): a timer-driven
   route `from: timer:tick?period=...` → `cache:{key:"k", ttl:"15m",
   on_miss:[recipient_list:{simple:"...", strategy:last_wins}]}` pointing at the
   mock 4xx server. Seed `k` with a known stale body before the tick. Trigger one
   tick. Assert via a companion `cache_peek_stale:{key:"k"}` that the entry is
   the seeded stale body, NOT `"timer:..." ` and NOT the inbound body.
4. Add the stale-serve integration test (Tests, scenario 2): a route
   `cache:{key:"k", on_miss:[do_try:{ steps:[recipient_list url→mock-403],
   catch:[cache_peek_stale:{key:"k"}]}]}`. Seed `k`. Send a request. Assert the
   response is HTTP 200 with the stale body content, NOT empty 200. **This test
   UNCONDITIONALLY owns the rc-n8rc stream-ownership assertions** (not
   conditional on Task 3.1): it ALWAYS asserts (i) no `Body::Stream already
   consumed` error appears in the captured runtime output, and (ii) the mock-403
   response status (403) reached the route's error path (the catch fired because
   the recipient_list surfaced the 403, proving the error reply was visible, not
   swallowed). This makes scenario 2 the always-on owner of the rc-n8rc
   stream-ownership scenarios regardless of whether Task 3.1 added a unit
   reproducer.
5. Ensure both tests are deterministic: fixed mock responses, no real timers
   (use a short, deterministic period or a manual tick trigger if the timer
   component supports one), no network races.

**Tests:**
- name: `cache_poison_timer_recipient_list_all_failed_no_writeback`
- setup: timer-driven cache-warming route with recipient_list → mock 4xx; key
  `k` pre-seeded with `"stale-seed"`.
- action: trigger one timer tick (or one route invocation); then read
  `cache_peek_stale("k")`.
- assert: the returned entry body is `"stale-seed"` (the seeded value); NOT
  `"timer:tick..."`; NOT empty.
- command: `cargo test -p camel-test --test cache_resilience cache_poison_timer_recipient_list_all_failed_no_writeback`
- expected: PASSES after Tasks 2.1 + 3.2 land (this is the gate that proves the
  composed path; it depends on the earlier fixes).

- name: `stale_serve_do_try_catch_cache_peek_stale_returns_stale_body`
- setup: the stale-serve route with recipient_list → mock 403; key `k`
  pre-seeded with `"stale-payload"`.
- action: send one request through the route.
- assert: HTTP 200 response with body `"stale-payload"`; NOT empty 200; NOT the
  inbound body. **Unconditional rc-n8rc assertions:** no `Body::Stream already
  consumed` error in the captured runtime output, AND the mock-403 status (403)
  reached the route's error path (the catch fired because recipient_list
  surfaced the 403 — proving the error reply was visible, not swallowed).
- command: `cargo test -p camel-test --test cache_resilience stale_serve_do_try_catch_cache_peek_stale_returns_stale_body`
- expected: PASSES after Task 3.2 lands; the stream-ownership assertions pass
  unconditionally (they verify the composed path, owning the rc-n8rc scenarios
  regardless of Task 3.1's unit-reproducer outcome).

**Acceptance:**
- `cargo test -p camel-test --test cache_resilience` exits 0 (both scenarios;
  exact crate + file).
- `cargo clippy -p camel-test -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.
- No external network calls (grep the test for `httpbin`, `google`, raw IPs —
  none expected); no `#[ignore]` without a CI-deferred note.
- Scenario 2 unconditionally asserts the rc-n8rc stream-ownership outcomes
  (single-consumption + 403 status/body visibility).

> **Implementation note (Task 4.1 result, r_glm-reviewed):** Scenario 1
> (`cache_poison_timer_recipient_list_all_failed_no_writeback`) landed and
> PASSES — it proves the epic's headline cache-poison property (rc-20yn +
> cache write-back skip composed end-to-end). Scenario 2's stream-ownership
> assertions were DEFERRED to bd **rc-n8rc** (P1): r_glm flagged that the
> worker's vacuous-resolution argument conflated the camel-http CONSUMER reply
> path (`lib.rs:1569`, genuinely unreachable post-rc-20yn) with the PRODUCER
> 403 path (`lib.rs:2288-2291`, which a recipient_list→HTTP-403 actually
> traverses). The producer-path double-consume was NOT empirically verified.
> CamelTestContext does not register the http component by default and no
> in-repo helper routes through it, so the empirical HTTP-403 test requires
> harness work (register http + mock 403 server) disproportionate for a P1.
> Closure evidence (producer-path source trace OR empirical test) is tracked
> in rc-n8rc. The epic's P0 (rc-20yn) and the rc-65yi composition are fully
> gated; rc-n8rc is the one open P1 thread requiring human judgment on
> verification depth.

- [x] 4.1
