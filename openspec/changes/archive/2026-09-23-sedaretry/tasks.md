# Tasks: sedaretry

## camel-component-seda

### Task 1.1: Typed terminal-config marker, predicate, and classifier exclusion

**Files:**
- `crates/components/camel-component-seda/src/lib.rs` (modified)

**Steps:**
1. RED first — add these failing tests in the existing `mod tests`
   characterization block (near the gate tests around line 2600):
   `genuine_config_reject_classifies_terminal`,
   `foreign_config_wording_imitation_stays_retryable`,
   `typed_with_foreign_source_not_terminal_config`,
   `foreign_terminal_marker_imitation_stays_retryable`,
   `terminal_config_marker_at_hop_limit_classifies`,
   `terminal_config_marker_beyond_limit_stays_retryable` (mirror the
   existing gate boundary tests' wrapper-chain construction for the
   at-8/inclusive and beyond-8 cases). Run
   `cargo test -p camel-component-seda --lib terminal_config` and
   confirm compile failure (predicate does not exist yet) — that is the
   expected RED.
2. Add crate-private marker enum below `NoActiveConsumersGate`
   (around line 557):
   `#[derive(Debug, Clone, PartialEq, Eq)] enum TerminalConfigError {
   MultipleConsumersWaitConflict }` with `Display` writing
   `"seda terminal-config-error rejection (multipleConsumers wait
   conflict)"` (non-canonical, never equal to any canonical rejection
   message) and an empty `impl std::error::Error`.
3. Add constructor `fn terminal_config_rejection() -> CamelError`
   beside `single_mode_gate_rejection` (around line 600): returns
   `CamelError::EndpointCreationFailedWithSource(canonical_detail,
   OpaqueErrorSource::new(Arc::new(TerminalConfigError::MultipleConsumersWaitConflict)))`
   where `canonical_detail` is the BYTE-IDENTICAL current wording
   from the reject site (lines ~1259-1263):
   `"multipleConsumers=true with waitForTaskToComplete != Never is not
   supported — a single request cannot have N valid replies without
   aggregator semantics"` (keep the existing multi-line string-literal
   folding so the rendered message is unchanged).
4. Extract the shared bounded walk as a generic private helper
   `fn marker_in_source_chain<T>(err: &CamelError) -> Option<T>` with
   bounds `T: std::error::Error + Clone + 'static`, mirroring
   `gate_from_error`'s loop (lines ~651-664): downcast-walk the
   `EndpointCreationFailedWithSource` source chain over
   `MAX_SOURCE_HOPS` hops with `unwrap_arc_dyn_error`, probing for
   `T` and returning a clone of the matched marker (both
   `NoActiveConsumersGate` and `TerminalConfigError` already derive
   `Clone`). Refactor `gate_from_error` to delegate to
   `marker_in_source_chain::<NoActiveConsumersGate>` (behavior
   identical); `terminal_config_from_error` becomes a thin wrapper:
   `marker_in_source_chain::<TerminalConfigError>(err).is_some()` —
   one walker, no drift.
5. Add pub predicate
   `pub fn is_seda_terminal_config_error(err: &CamelError) -> bool`
   beside `is_no_active_consumers_gate` (line ~680) with a doc comment
   following the gate predicate's doctrine paragraph (typed
   provenance, bounded walk, Display never classifies, deterministic
   config conflict can never succeed on retry).
6. Narrow `is_direct_startup_race` (line ~700): the
   `EndpointCreationFailedWithSource(..)` arm becomes
   `!(is_no_active_consumers_gate(err) || is_seda_terminal_config_error(err))`.
   Update its doc comment (lines ~684-699) to record the second
   exclusion.
7. Convert the reject site (lines ~1258-1264):
   `if state.config.multiple_consumers && should_wait { return
   Err(terminal_config_rejection()); }` — behavior, check order, and
   rendered message unchanged.
8. Leave the existing synthetic-wording test
   `other_seda_wordings_stay_retryable` (line ~2987) UNCHANGED — it
   pins the delta scenarios "other SEDA endpoint-creation failures
   stay retryable" and "the plain variant never classifies as a
   terminal-config error". Pin the reject-site wording instead by
   adding an `assert_endpoint_failure_payload(err, "multipleConsumers=true
   with waitForTaskToComplete != Never is not supported — a single
   request cannot have N valid replies without aggregator semantics")`
   assertion inside `genuine_config_reject_classifies_terminal` (the
   helper at lib.rs ~28-31 accepts both endpoint-creation variants).
9. GREEN: `cargo test -p camel-component-seda --lib` passes — the six
   new tests plus every existing gate/race/residual test unchanged.

**Tests:** (executable spec)
- `genuine_config_reject_classifies_terminal`: a fanout endpoint
  (`seda:q?multipleConsumers=true`) with a STARTED consumer (reuse the
  consumer-start pieces from `has_active_consumer_tracks_consumer_lifecycle`
  at line ~3050 / `started_consumer_ctx()` at line ~1916) and a producer
  from `seda:q?multipleConsumers=true&waitForTaskToComplete=Always`
  (capture pattern from `producer_gate_wording_fanout_mode` at line
  ~2664) → capture the producer rejection → assert
  `is_seda_terminal_config_error(&e)` is true AND
  `is_direct_startup_race(&e)` is false AND
  `assert_endpoint_failure_payload(err, canonical_detail)` passes with
  `canonical_detail` = the string literal from the reject site
  (`multipleConsumers=true with waitForTaskToComplete != Never is not
  supported — a single request cannot have N valid replies without
  aggregator semantics`) — byte-identical rendered message.
- `foreign_config_wording_imitation_stays_retryable`: plain
  `CamelError::EndpointCreationFailed` carrying the byte-exact
  canonical config wording (copy the string literal from the reject
  site) → assert `!is_seda_terminal_config_error(&e)` AND
  `is_direct_startup_race(&e)`.
- `typed_with_foreign_source_not_terminal_config`:
  `EndpointCreationFailedWithSource` with a foreign `impl Error` source
  → assert `!is_seda_terminal_config_error(&e)` AND
  `is_direct_startup_race(&e)`.
- `foreign_terminal_marker_imitation_stays_retryable`:
  `EndpointCreationFailedWithSource` whose source is a LOCAL test-only
  marker type (mimicking a foreign crate's marker) → assert
  `is_direct_startup_race(&e)` (the seda walk only matches the seda
  type).
- `terminal_config_marker_at_hop_limit_classifies`: wrap the marker in
  a chain of exactly 8 source hops (mirror the gate at-limit test
  construction) → assert `is_seda_terminal_config_error(&e)` AND
  `!is_direct_startup_race(&e)`.
- `terminal_config_marker_beyond_limit_stays_retryable`: marker deeper
  than 8 hops → assert `!is_seda_terminal_config_error(&e)` AND
  `is_direct_startup_race(&e)`.

**Acceptance:**
- `cargo test -p camel-component-seda --lib` exits 0.
- `cargo clippy -p camel-component-seda --all-targets -- -D warnings`
  exits 0.
- `cargo fmt --check` clean for the crate.
- The genuine-reject test asserts BOTH predicates; the rendered
  rejection text is byte-identical to the pre-change message.

- [x] 1.1

## camel-cli

### Task 1.2: CLI classification tests and doc-comment updates

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/src/commands/job/startup_retry_classification_tests.rs` (modified)

**Steps:**
1. RED first (ordered AFTER Task 1.1 so the predicate exists): add
   `behavioral_multiple_consumers_wait_config_fails_fast`
   to `startup_retry_classification_tests.rs` (spec scenario "SEDA
   terminal-config error fails fast"). Before Task 1.1 lands the RED
   is a compile failure (`is_seda_terminal_config_error` does not
   exist); with 1.1 reverted it would burn the full 3 s window before
   the classification assert fails. Run
   `cargo test -p camel-cli --lib behavioral_multiple_consumers` after
   1.1 is in — expected green immediately; the RED form is verified
   only if 1.1 is reverted.
2. Build the test: boot `CamelContext` with `SedaComponent` (reuse
   `booted_seda_context`), create endpoint
   `seda:q?multipleConsumers=true`, start ONE consumer on it (so the
   fanout endpoint has an active consumer and the pre-enqueue gate
   passes), then call
   `send_with_startup_retry(&ctx, &tick_send("seda:q?multipleConsumers=true"),
   "seda:q?multipleConsumers=true&waitForTaskToComplete=Always", &[])`
   — the send URI mirrors the job loop's forced Always (the
   `document::seda_send_uri` output shape).
3. Assert: the result is `Err(SendError::Pipeline(e))`;
   `camel_component_seda::is_seda_terminal_config_error(&e)` is true;
   `is_retryable_startup_failure(&e)` is false; and the send returns
   promptly — `started.elapsed() < Duration::from_secs(1)` (the old
   classification burned the full 3 s `SEND_RETRY_WINDOW`; measure
   `started` immediately before the call).
4. GREEN comes from Task 1.1 (no CLI code change); if 1.1 landed first,
   this test must pass immediately — do not modify
   `is_retryable_startup_failure` logic.
5. Update the doc comments on `is_retryable_startup_failure` (lines
   ~1950-1976) and `send_with_startup_retry` (lines ~1981-1991) in
   `mod.rs`: record the terminal-config exclusion alongside the gate
   (deterministic configuration conflict, typed marker, first-attempt
   return without sleeping). No logic changes.
6. Run the whole classification suite:
   `cargo test -p camel-cli --lib startup_retry` — every existing
   characterization row (gate fail-fast, foreign imitations retryable,
   direct race retryable, queue-full retryable) must still pass
   unchanged.

**Tests:** (executable spec)
- `behavioral_multiple_consumers_wait_config_fails_fast`: started
  fanout consumer + forced-Always send → `SendError::Pipeline` on the
  first attempt, `is_seda_terminal_config_error` true,
  `is_retryable_startup_failure` false, elapsed < 1 s.
- Every existing test in `startup_retry_classification_tests.rs`
  passes unchanged (regression guard).

**Acceptance:**
- `cargo test -p camel-cli --lib startup_retry` exits 0.
- `cargo clippy -p camel-cli --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` clean.
- Zero logic changes in `mod.rs` (doc comments only) —
  `git diff crates/camel-cli/src/commands/job/mod.rs` shows comment
  lines only.

- [x] 1.2

### Task 1.3: Subprocess e2e regression — terminal-config job exits Failed fast

**Files:**
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified)

**Steps:**
1. Add `seda_terminal_config_job_fails_fast` beside
   `seda_gate_side_effect_executes_exactly_once` (line ~550), following
   its harness exactly: tempdir, `write_config`, routes dir, job doc.
2. Route (fanout consumer must be ACTIVE for the config reject to be
   reached — the gate check runs first):
   `routes: [{id: "job-config", from: "seda:q?multipleConsumers=true",
   steps: [{to: "log:done"}]}}]`; job doc send:
   `to: "seda:q?multipleConsumers=true"`, `mode: one-shot`,
   `timeout: 60s`.
3. Run via `run_job` and assert: exit code 1; parsed JSON report
   `outcome == "Failed"`; `report["error"]` contains the substring
   `multipleConsumers=true with waitForTaskToComplete != Never`; and
   `report["duration_ms"].as_u64() < 1500` (spec bound: half the 3 s
   send retry window; `duration_ms` is `started.elapsed()` frozen at
   report construction, BEFORE the shutdown path — the 5 s
   `MIN_SHUTDOWN_BUDGET` cannot inflate it, which is why the bound
   discriminates: a retry burn alone adds ≥ 3000 ms — if boot time on
   this machine makes 1500 infeasible, STOP and report
   `test-design-gap: measured boot <N> ms exceeds the 1.5 s budget`
   instead of loosening the bound).
4. Run `cargo test -p camel-cli --test job_one_shot_test seda_terminal_config`
   — passes; then the full file
   `cargo test -p camel-cli --test job_one_shot_test` — every existing
   test unchanged (the gate exactly-once test included).

**Tests:** (executable spec)
- `seda_terminal_config_job_fails_fast`: active fanout consumer route +
   direct send to `seda:q?multipleConsumers=true` → exit 1, outcome
   `Failed`, error names the multipleConsumers+wait conflict,
   `duration_ms` < 1500.

**Acceptance:**
- `cargo test -p camel-cli --test job_one_shot_test` exits 0.
- `cargo clippy -p camel-cli --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` clean.

- [x] 1.3
