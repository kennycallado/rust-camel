# Tasks: idemon

Single-phase change (no `## Phase N` groups; design.md records no Phases section).
Order: 1.1 → 1.2 → 1.3 → 1.4 → 2.1 → 2.2 → 3.1 → 3.2.

## camel-processor: reservation semantics

### Task 1.1: OnMissingKey enum, empty→absent normalization, missing-key behavior

**Files:**
- `crates/camel-processor/src/idempotent_consumer.rs` (modified)
- `crates/camel-processor/src/lib.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified)
- `crates/camel-core/src/idempotent/redb_repository.rs` (modified)

**Steps:**
1. Add `pub enum OnMissingKey { Fail, Process }` (derives `Debug, Clone, Copy, PartialEq, Eq`; `impl Default for OnMissingKey { fn default() -> Self { OnMissingKey::Fail } }`) in `idempotent_consumer.rs`, with a doc comment citing the spec requirement "Idempotent consumer step reservation semantics"; extend the crate-root `pub use idempotent_consumer::{...}` in `lib.rs` (`:93`) with `OnMissingKey` (camel-core call sites reference `camel_processor::OnMissingKey`).
2. Normalize the sync arm inside `MessageIdSource::message_id`: `Self::Sync(f) => Ok(f(exchange).filter(|s| !s.is_empty()))` — an empty string from a sync extractor is treated exactly like an absent key (the async arm already maps `Value::String(s) if s.is_empty() => None`; leave that arm as-is). Update the `MessageIdSource` doc comment: keyless exchanges are resolved by `on_missing_key`, not silently forwarded (Apache Camel parity is deliberately dropped — see ADR-0099).
3. Add fields `on_missing_key: OnMissingKey` to `IdempotentConsumerSegment`; extend `IdempotentConsumerSegment::new` with an `on_missing_key: OnMissingKey` parameter (before any other signature changes — tasks 1.2/1.3 add more); mirror the field in `impl Clone`.
4. In `OutcomePipeline::run`, replace the `Ok(None) => return self.child_pipeline.run(exchange).await` arm: under `OnMissingKey::Fail` return `PipelineOutcome::Failed(CamelError::ProcessorError(format!("idempotent_consumer: no idempotency key on the exchange and on_missing_key is 'fail' (repository '{}')", self.repository.name()))` — child MUST NOT run; under `OnMissingKey::Process` emit `tracing::warn!(repository = %self.repository.name(), "idempotent_consumer: no idempotency key on the exchange; processing without idempotency (on_missing_key = 'process')")` and forward to the child.
5. Update all existing call sites mechanically (keep behavior identical): `build_segment` test helper and both direct `IdempotentConsumerSegment::new` calls in this file's tests get `OnMissingKey::Fail`; `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (`:129`) and `crates/camel-core/src/idempotent/redb_repository.rs` (`:498`) pass `camel_processor::OnMissingKey::Fail` (real DSL wiring arrives in task 2.1).

**Tests:** (all in `crates/camel-processor/src/idempotent_consumer.rs` `#[cfg(test)]`; command `cargo test -p camel-processor --lib`)
- `missing_key_fail_is_default_sync_arm`: MockRepo empty, sync extractor returning `None` (exchange without messageId header), segment built with `OnMissingKey::Fail` → run → assert `matches!(outcome, PipelineOutcome::Failed(ref e) if e.to_string().contains("idempotent_consumer") && e.to_string().contains("mock"))` and child NOT invoked and `repo.contains_key("")` is false.
- `missing_key_fail_is_default_async_arm`: same repo/child, `MessageIdSource::Async` returning `Value::Null` → run → same Failed assertion, child NOT invoked.
- `empty_key_treated_as_absent_sync_arm`: sync extractor returning `Some("")`, `OnMissingKey::Fail` → run → Failed naming step+repo, child NOT invoked, repo stays empty (asserts the `Some("")` real-key collision from rc-v67ye is gone).
- `empty_key_treated_as_absent_async_arm`: `MessageIdSource::Async` returning `Value::String("")`, `OnMissingKey::Process` → run → child invoked, repo never consulted (use `MockRepo::failing_contains()` so any repo access errors loudly and fails the assertion), outcome Completed.
- `missing_key_process_forwards_with_warning`: two segments, both `OnMissingKey::Process`, both `MockRepo::failing_contains()` — one with a sync extractor returning `None`, one with an async expression evaluating to `Value::Null` → each run: child invoked, outcome Completed (proves the repository was untouched on both arms); under a tracing capture helper (repo precedent: `capture_warns`, `crates/camel-processor/src/data_format/test_util.rs:74`; guard convention in `docs/testing/tracing-capture-guards.md`) each run records a WARN log (assert exactly one warn per run; `cargo test -p camel-processor --lib`).

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-processor -p camel-core --all-targets -- -D warnings` exits 0
- The child never runs on a missing key under `Fail` (all three fail-mode tests assert `!child_invoked`)

- [x] 1.1

### Task 1.2: Duplicate observability and bookkeeping metrics

**Files:**
- `crates/camel-processor/src/idempotent_consumer.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified)
- `crates/camel-core/src/idempotent/redb_repository.rs` (modified)

**Steps:**
1. Add field `rt: Arc<dyn camel_component_api::RuntimeObservability>` to `IdempotentConsumerSegment` (same seam as `CacheService` — see `crates/camel-processor/src/cache_eip.rs:178`); extend `IdempotentConsumerSegment::new` with `rt` as the LAST parameter; mirror in `impl Clone`.
2. Add `pub const CAMEL_IDEMPOTENT_DUPLICATE: &str = "camel.idempotent.duplicate";` in `idempotent_consumer.rs` with a doc comment naming the spec requirement "Duplicate skips and bookkeeping failures are observable".
3. Duplicate path (`contains == Ok(true)`): set `exchange.set_property(CAMEL_IDEMPOTENT_DUPLICATE, Value::Bool(true))` (requires making the run future own a mutable `exchange` — it already does), then `self.rt.metrics().record_counter("camel.idempotent.duplicates", 1.0_f64, &[("repository", self.repository.name())])` annotated with `// allow-open-label rc-36ujy` on the preceding line (metric-labels lint: repository is a runtime value), then return `Completed(exchange)`.
4. Eager race path (`add == Ok(false)` under `self.eager`): same property + same counter + `Completed(exchange)`.
5. Non-eager post-success add failure (current `:183`): change `tracing::warn!` to `tracing::error!` (same fields) and add `self.rt.metrics().record_counter("camel.idempotent.add_failures", 1.0_f64, &[("repository", self.repository.name())])` with `// allow-open-label rc-36ujy`; the outcome stays `Completed`.
6. Update test call sites: the `build_segment` helper and the direct `new` in `message_id_async_error_propagates` take `Arc::new(NoopRuntimeObservability)` (import `camel_component_api::test_support::NoopRuntimeObservability`; `camel-processor` already enables the `test-support` feature — see `crates/camel-processor/Cargo.toml:41`); `crates/camel-core` call sites from task 1.1 gain `ctx.rt.clone()` (step compiler) and `Arc::new(NoopRuntimeObservability)` (redb test, feature `test-support` available in camel-core dev-deps — verify; if absent, construct a trivial no-op `RuntimeObservability` impl in that test module).

**Tests:** (`cargo test -p camel-processor --lib`)
- `duplicate_contains_hit_sets_property_and_counter`: repo pre-added "dup-1", metrics collector that records counters (add a `RecordingMetrics` test double implementing `camel_api::metrics::MetricsCollector` following the `CounterRecording` pattern in `crates/camel-processor/src/cache_eip.rs:2121`) → run → outcome `Completed`; extract the exchange from the outcome and assert `ex.property(CAMEL_IDEMPOTENT_DUPLICATE) == Some(&Value::Bool(true))`; assert the recorded counters contain `("camel.idempotent.duplicates", repository="mock")` exactly once; child NOT invoked.
- `eager_race_duplicate_sets_property_and_counter`: eager segment with a MockRepo variant where `contains` returns `Ok(false)` but `add` returns `Ok(false)` even for an absent key (a `force_add_duplicate: bool` flag — do NOT pre-add the key: pre-adding would take the `contains`-hit path instead of the race path) → run → assert BOTH operations occurred (op log shows `contains` then `add`), `Completed` with the property set, the duplicate counter incremented once, child NOT invoked.
- `non_eager_add_failure_keeps_success_with_error_metric`: non-eager segment, child returns `Completed`, MockRepo variant whose `add` returns `Err` (add a `fail_add: bool` flag to `MockRepo`) → run → outcome `Completed` (NOT Failed), counter `camel.idempotent.add_failures` incremented once, key absent from repo; under the tracing capture helper (`capture_warns` precedent, `crates/camel-processor/src/data_format/test_util.rs:74`) assert an ERROR-level log was emitted (pins the `:183` level upgrade — the test fails while the log stays at warn).

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-processor -p camel-core --all-targets -- -D warnings` exits 0
- Both duplicate paths and the `:183` path are asserted via recorded counters, not log text

- [x] 1.2

### Task 1.3: Pending-rollback state machine — failed rollback, recovery, marker lifetime

**Files:**
- `crates/camel-processor/src/idempotent_consumer.rs` (modified)

**Steps:**
1. Add `#[derive(Default)] struct PendingRollbacks { pending: HashSet<String>, in_flight: HashSet<String> }` (private, `std::collections::HashSet`) and a field `rollback_state: Arc<std::sync::Mutex<PendingRollbacks>>` on `IdempotentConsumerSegment`; constructed fresh in `new`, `Arc::clone`d in `impl Clone` (shared across clones). Doc comment: single critical sections, never hold the lock across an `.await`.
2. Failed eager rollback (current `:191-199` arm): on `repo.remove` failure — insert the key into `pending` under the lock; keep the child's error VERBATIM as the returned `PipelineOutcome::Failed` (do NOT wrap: `on_exceptions` matches structurally, see `camel-dsl/src/compile.rs:999`); emit `tracing::error!` carrying BOTH errors as fields (`child_error = %child_err, rollback_error = %remove_err, repository, key`) stating the key is queued for pending-rollback recovery.
3. Run-start recovery, BEFORE the contains check (single critical section, then drop the guard before any `.await`): if `in_flight.contains(key)` → return `PipelineOutcome::Failed(CamelError::ProcessorError(format!("idempotent_consumer: key is under pending-rollback recovery (repository '{}')", self.repository.name())))` — no repository ops, child MUST NOT run; else if `pending.remove(key)` succeeds → this clone owns recovery: move the key to `in_flight` under the SAME critical section, drop the guard, then `repo.remove(key)`: `Err(remove_err)` → move `in_flight → pending` under the lock and return `Failed(remove_err)`; `Ok(())` → fall through to the normal contains/reserve/child flow with `in_flight` STILL HELD.
4. Marker lifetime (clear when the reservation step resolves; never leave a settled key in flight): eager mode — after the pre-child `repo.add` resolves (any of `Ok(true)` reserved / `Ok(false)` race duplicate / `Err`), remove the key from `in_flight` under the lock before returning or continuing (race duplicate returns the property+counter `Completed` from task 1.2; add error returns `Failed`); lazy mode — after the child outcome resolves and the post-child bookkeeping resolves (`add` success, `:183` failure with success kept, or a non-`Completed` outcome where no add happens), remove the key from `in_flight` under the lock. Implementation shape: a small `fn clear_in_flight(&self, key: &str)` helper called at each resolution point (fail paths AND success paths — a missing clear on any early return is a wedge bug; walk every return site in `run`).
5. Re-entry recovery run (owner path after successful removal): the normal flow reserves and runs the child exactly once — the delivery is never a duplicate and never a zero-work success (this is asserted by the tests below via a second `run` call on a cloned segment).

**Tests:** (`cargo test -p camel-processor --lib`)
- `failed_rollback_preserves_child_error_and_logs_context`: eager + `remove_on_failure = true`, child returns `Failed(CamelError::Io("redis down".into()))`, MockRepo variant whose `remove` returns `Err` (extend the `fail_add` flag pattern to `fail_remove`) → run → assert `matches!(outcome, PipelineOutcome::Failed(CamelError::Io(msg)) if msg.contains("redis down"))` (typed regression: structural `on_exceptions` matching survives), key present in `pending` (assert via a `#[cfg(test)]` accessor or by observing the next run's recovery behavior); under the tracing capture helper assert an ERROR-level log whose fields carry both `child_error` and `rollback_error` (spec Requirement: structured context).
- `pending_removal_fails_again_fails_without_repo_or_child`: seed `pending` with "k" (drive the previous scenario), then on the SAME segment run a second delivery with key "k" against a repo whose `remove` still fails → outcome `Failed` with the removal error; assert the child was NOT invoked and the repo's `contains`/`add` were NOT called (add call-recording vectors to `MockRepo`: `ops: Mutex<Vec<(&'static str, String)>>`), and "k" remains recoverable (subsequent successful recovery works — next test).
- `pending_rollback_retried_then_child_runs_exactly_once`: seed `pending` with "k", repo `remove` now succeeds → second delivery: assert child invoked EXACTLY once, outcome equals the child's outcome, key NOT reported as duplicate (no duplicate property on the outcome exchange), `in_flight` empty after the run (no-wedge: a third delivery with "k" follows the normal duplicate path and returns `Completed` with the property set — proving the marker was cleared, not leaked).
- `marker_cleared_on_eager_reservation_resolution`: SEED recovery first (drive a failed rollback whose removal succeeds on retry — the marker is now in `in_flight`), then eager mode: reservation resolves `Ok(true)`, child returns `Stopped` → assert a follow-up delivery with the same key does NOT hit the recovery-in-progress error (marker was cleared at reservation resolution; it follows the normal `contains == true` duplicate path with the duplicate property set).
- `marker_cleared_on_lazy_bookkeeping_resolution`: same seeding, lazy mode (`eager: false`): child returns `Completed`, post-success `add` resolves → assert a follow-up delivery does NOT hit the recovery-in-progress error and follows the normal duplicate path (marker cleared when the lazy bookkeeping resolved).
- `in_flight_failure_paths_clear_the_marker`: seed recovery as above; eager run where the pre-child `add` returns `Err` → outcome `Failed`; follow-up delivery does NOT hit the recovery-in-progress error (asserts the fail-path clear from step 4 — no wedged in-flight key).
- `eager_rollback_then_retry_runs_child_again`: eager + `remove_on_failure = true`, child fails (removal succeeds) → a second delivery with the same key runs the child a second time (NOT duplicate-skipped) — pins the "eager rollback default" scenario end to end.
- `eager_without_rollback_keeps_at_most_once_with_observable_skip`: eager + `remove_on_failure = false`, child fails → key retained; a second delivery with the same key returns `Completed` with the `CAMEL_IDEMPOTENT_DUPLICATE` property set and the duplicate counter incremented (observable skip, per the explicit opt-in contract).

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0
- No `std::sync::Mutex` guard is held across an `.await` (compiles without `Send` errors on the boxed future — the existing `Box::pin(async move)` future must stay `Send`)

- [x] 1.3

### Task 1.4: Raced-delivery loud failure — deterministic pause regressions

**Files:**
- `crates/camel-processor/src/idempotent_consumer.rs` (modified)

**Steps:**
1. Make the concurrency fixtures clone-safe and deterministic. The existing `ScriptedChild::clone_box` panics (`unreachable!`, `:290`) and segment cloning (tasks 1.3/1.4 clone segments for A/B) calls it — replace it with a clone-safe child: `invoked: Arc<AtomicUsize>` shared by all clones, `clone_box` constructs a new `ScriptedChild` sharing the SAME `Arc`, invocations counted with `fetch_add(1, SeqCst)` for exact counts (a boolean cannot express "exactly once"), and repo state stays shared through the existing `Arc<MockRepo>`. Exactly-once assertions read the shared counter AFTER both runs complete.
2. Extend the test `MockRepo` with TWO independent gates, each with an entered-handshake: a `remove` gate and an `add` gate (`tokio::sync::watch` or `oneshot` pairs; when armed, the op RECORDS its op-log entry, signals "entered", then awaits release). Each test configures its OWN gate mutation — the during-removal test arms the `remove` gate only; the after-removal test arms the `add` gate only (separate mutations: premature marker clearing after removal fails the second test but NOT the first). Every await in these tests is wrapped in `tokio::time::timeout` (bounded; `cargo xtask lint-unbounded-wait` stays green). The gates exist ONLY in tests — no production code changes beyond task 1.3.
3. Test the during-removal window: seed `pending` with "k"; clone the segment into A and B (shared `rollback_state`, shared gated repo); start A's `run` and await its "entered" handshake (deterministic: A is inside the gated `remove`); run B's `run` with the same key to completion (bounded); then release the gate and await A. Assert: B's outcome is `Failed` containing "pending-rollback recovery", B's child NOT invoked, and B performed NO repository operations (op log contains only A's `remove` at the moment B returned); A then reserves and runs the child exactly once (shared counter == 1); a final same-key delivery returns the normal observable duplicate `Completed`.
4. Test the after-removal before-reservation window (detects premature marker clearing): seed `pending` with "k"; A takes ownership, removal succeeds, A blocks inside the pre-child reservation `add` (await the add-gate "entered" handshake); B runs with the same key → B must return `Failed` naming recovery-in-progress with NO `contains`/`add` ops and no child (if the marker were cleared at removal time, B would reserve and win the race — this test fails on that regression); release the gate; A's `add` resolves `Ok(true)`; assert A runs the child exactly once and B never ran a child.
5. Confirm the exercised interleavings are deterministic (entered-handshakes + `tokio::join!`/bounded timeouts, no sleeps — `cargo xtask lint-test-sleep` stays green).

**Tests:** (`cargo test -p camel-processor --lib`)
- `raced_delivery_during_removal_fails_loudly`: (step 2) → exact assertions as listed.
- `raced_delivery_after_removal_before_reservation_fails_loudly`: (step 3) → exact assertions as listed.

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0, with separate mutation verification: mutant A (clear the marker BEFORE the recovery removal) must fail the during-removal test (it may also fail the after-removal test — the marker is gone for both windows); mutant B (clear the marker AFTER the removal but BEFORE the reservation) must fail the after-removal test while the during-removal test still PASSES (that exclusivity is what pins the marker to span removal→reservation); verify both locally, then restore
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0
- `cargo xtask lint-test-sleep` exits 0 (no sleeps in tests)
- `cargo xtask lint-unbounded-wait` exits 0 (all gate awaits bounded)

- [x] 1.4

## camel-dsl + camel-core: transport and policy

### Task 2.1: DSL surface — on_missing_key field, eager-aware rollback default, load-time warning

**Files:**
- `crates/camel-processor/src/idempotent_consumer.rs` (modified)
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/model.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified)
- `crates/camel-core/src/startup_validation.rs` (modified)
- `crates/camel-core/tests/expression_failure_propagation_test.rs` (modified)

**Steps:**
1. In `camel-processor/src/idempotent_consumer.rs`: add `pub fn parse(raw: Option<&str>) -> Result<OnMissingKey, CamelError>` on `OnMissingKey`, mirroring the canonical `PeekStaleMissPolicy::parse_on_miss` pattern (`crates/camel-processor/src/cache_eip.rs:785`): `None → Ok(Fail)`, `Some("fail") → Ok(Fail)`, `Some("process") → Ok(Process)`, anything else → `Err(CamelError::Config(format!("idempotent_consumer: invalid on_missing_key '{}'; must be \"fail\" or \"process\"", value)))` — the error NAMES the field and the accepted values (a bare serde enum names the accepted variants but not the field; repo precedent `route_ast.rs:2330`).
2. In `route_ast.rs`: add `#[serde(default)] pub on_missing_key: Option<String>` to `IdempotentConsumerBody` with `#[cfg_attr(feature = "schema", schemars(extend("enum" = serde_json::json!(["fail", "process", null]))))]` (the `CachePeekStaleBody.on_miss` schema pattern, `route_ast.rs:1480`); update the `remove_on_failure` field doc comment to "When `eager`, remove the key if the child fails (default: `true` under `eager`, `false` otherwise). An explicit `false` under `eager` keeps an at-most-once contract and emits a load-time warning."; `deny_unknown_fields` stays. NO DSL enum type is introduced (a bare serde enum's unknown-variant error names the accepted variants but not the field, so resolution uses `OnMissingKey::parse`, which names both).
3. In `model.rs`: add `pub on_missing_key: Option<String>` to `IdempotentConsumerStepDef` (`:606`) — the raw string transports through the programmatic path; resolution happens at compile; update the stale `remove_on_failure` doc on `IdempotentConsumerStepDef` (`:617`, "Default `false`") to the eager-aware default text.
4. In `yaml.rs` (`RouteDslStep::IdempotentConsumer` conversion, `:1715`): transport `body.on_missing_key` into the `IdempotentConsumerStepDef` raw field — NO parsing at conversion; `compile_idempotent_consumer_step` is the single resolution point (the JSON path shares the serde model; verify `json.rs` needs no separate mapping).
5. In `compile.rs` `compile_idempotent_consumer_step` (`:1620`): FIRST call `OnMissingKey::parse(def.on_missing_key.as_deref())?` (rejects invalid values with the field-named error on the programmatic path too — replaces the earlier "no dead check" note); add the private pure resolver `fn resolve_remove_on_failure(eager: bool, explicit: Option<bool>) -> (bool, bool)` returning `(effective_value, warn_at_most_once)` with the mapping: `(true, None) → (true, false)`, `(true, Some(v)) → (v, v == false)`, `(false, _) → (explicit.unwrap_or(false), false)`; when `warn_at_most_once` emit `tracing::warn!("idempotent_consumer: eager=true with explicit remove_on_failure=false keeps an at-most-once contract — a failed child leaves the key reserved and retries skip it as duplicates")`; forward the parsed `OnMissingKey` into `BuilderStep::IdempotentConsumer`.
6. In `route_definition.rs`: add `pub on_missing_key: camel_processor::OnMissingKey` to `BuilderStep::IdempotentConsumer` (`:357`; camel-core already depends on camel-processor — `step_compilers/core.rs:129`); update the Debug-format test at `:1739` (constructor gains `on_missing_key: camel_processor::OnMissingKey::Fail`; expected string gains `on_missing_key: Fail`).
7. In `step_compilers/core.rs` (`BuilderStep::IdempotentConsumer` arm, `:98`): destructure `on_missing_key` and pass it plus `ctx.rt.clone()` into `IdempotentConsumerSegment::new`; update the unit-test construction at `:1010` with the new field.
8. Update the remaining explicit `BuilderStep::IdempotentConsumer` construction sites (both break the build when the field lands): `crates/camel-core/src/startup_validation.rs` (`:616`, test `scheme_scanner_detects_exec_in_idempotent_consumer`) and `crates/camel-core/tests/expression_failure_propagation_test.rs` (`:387`) — add `on_missing_key: camel_processor::OnMissingKey::Fail`.

**Tests:** (camel-dsl: `cargo test -p camel-dsl --lib`; camel-core: `cargo test -p camel-core --lib` and `cargo test -p camel-core --test expression_failure_propagation_test`; camel-processor: `cargo test -p camel-processor --lib`)
- `on_missing_key_parse_matrix`: unit test `OnMissingKey::parse` — `None → Fail`, `Some("fail") → Fail`, `Some("process") → Process`, `Some("skip") → Err` whose message contains `on_missing_key`, `skip`, and both accepted values.
- `dsl_accepts_on_missing_key_fail_and_process`: parse two YAML routes setting `on_missing_key: "fail"` / `"process"` → both parse; compiled `BuilderStep::IdempotentConsumer` carries the mapped `OnMissingKey`.
- `dsl_rejects_invalid_on_missing_key_value`: route with `on_missing_key: "skip"` → parse error whose message contains `on_missing_key` and the accepted values (passes because resolution uses `OnMissingKey::parse`, not serde's variant error).
- `dsl_rejects_unknown_idempotent_field`: route with `on_duplicate: true` inside `idempotent_consumer` → parse error (unknown field).
- `dsl_default_on_missing_key_is_fail`: route omitting the field → compiled step carries `OnMissingKey::Fail`.
- `resolve_remove_on_failure_matrix`: unit test the pure resolver — `(true, None) → (true, false)`, `(true, Some(true)) → (true, false)`, `(true, Some(false)) → (false, true)`, `(false, None) → (false, false)`, `(false, Some(true)) → (true, false)`, `(false, Some(false)) → (false, false)`.
- `eager_explicit_false_warns_at_compile`: compile a route with `eager: true` + explicit `remove_on_failure: false` under a tracing capture helper (repo precedent: `capture_warns` in `crates/camel-dsl/src/discovery.rs:2708` and `crates/camel-processor/src/data_format/test_util.rs:74`; guard convention in `docs/testing/tracing-capture-guards.md`) → captured logs contain a WARN naming `idempotent_consumer` and the at-most-once contract.
- `eager_absent_remove_on_failure_compiles_effective_true`: compile a route with `eager: true` and NO `remove_on_failure` → the compiled `BuilderStep::IdempotentConsumer` carries `remove_on_failure: true` (pins the eager-rollback-default wiring end to end; catches a resolver that is never called).
- `builder_step_debug_includes_on_missing_key`: the updated `route_definition.rs` Debug test passes.

**Acceptance:**
- `cargo test -p camel-dsl --lib`, `cargo test -p camel-core --lib`, and `cargo test -p camel-core --test expression_failure_propagation_test` exit 0
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-dsl -p camel-core -p camel-processor --all-targets -- -D warnings` exits 0
- `deny_unknown_fields` discipline preserved (unknown-field test passes); invalid VALUE rejected with a field-named error

- [x] 2.1

### Task 2.2: Schema and TS type regeneration

**Files:**
- `schemas/dsl/route-schema.json` (modified, generated)
- `schemas/ts/IdempotentConsumerBody.ts` (modified, generated)
- `crates/camel-lint/schema/route-schema.json` (modified, generated)

**Steps:**
1. Run `cargo xtask schema` in the worktree — regenerates the DSL JSON schema, TS types, and the camel-lint embedded copy.
2. Inspect the diff: `IdempotentConsumerBody` gains `on_missing_key` (`"default": null`, extend-based enum `["fail", "process", null]` — the `CachePeekStaleBody.on_miss` rendering pattern), the `remove_on_failure` description reflects the eager-aware default; no OTHER schema content drifts. (No new TS type file: the field is `Option<String>` with a schema enum extension, not a Rust enum — `OnMissingKeyDsl.ts` does not exist.)
3. Run `cargo xtask schema --check` — must exit 0 (idempotent regen).

**Tests:**
- `cargo xtask schema --check` exits 0.
- `grep -c "on_missing_key" schemas/dsl/route-schema.json` ≥ 1 and the `IdempotentConsumerBody` schema block contains the enum values `fail` and `process`.

**Acceptance:**
- `cargo xtask schema --check` exits 0 (this gate is NOT N/A — a DSL field was added)
- Schema diff contains only idempotent_consumer-related changes

- [x] 2.2

## docs

### Task 3.1: ADR-0099 — idempotent reservation semantics

**Files:**
- `docs/adr/0099-idempotent-reservation-semantics.md` (new)

**Steps:**
1. Write the ADR following the format of `docs/adr/0098-redis-value-contract.md` (Status/Context/Decision/Consequences). Content: fail-closed missing keys (`on_missing_key` default `fail`, empty == absent, both arms); eager rollback default (`remove_on_failure` defaults `true` under `eager`; explicit `false` = opt-in at-most-once + load-time warning); pending-rollback rule (shared lock-protected pending/in-flight state, single-owner recovery, in-flight marker spans removal→reservation resolution, raced deliveries fail loudly); "a retry must never pass as a duplicate" (bounded to pending recovery). 
2. Cite ADR-0023 (Contract C1), ADR-0032 (untrusted exchange data — keys and redelivery headers), ADR-0090 (loud failure over silent fallback).
3. State the deliberate divergence from Apache Camel (missing key silently disables idempotency there; here it fails by default) and why: without this ADR someone will later "restore parity".
4. Check `docs/adr/` numbering — 0099 must be the next free number at implementation time (verify with `ls docs/adr/ | tail -3`; if another mission took 0099, renumber and update tasks).

**Tests:**
- `cd docs && mdbook build` exits 0 (ADR is wired by the adr list; verify the page renders).
- No lint-context-citations violation: `cargo xtask lint-context-citations` exits 0.

**Acceptance:**
- ADR cites 0023, 0032, 0090 by relative link
- `cargo xtask lint-context-citations` exits 0

- [x] 3.1

### Task 3.2: Guide updates and architecture refresh — step semantics and eager defaults

**Files:**
- `docs/src/eip/idempotent-consumer.md` (modified)
- `docs/src/yaml-dsl/step-verbs/resilience.md` (modified)
- `CONTEXT-MAP.md` (modified)
- `crates/camel-processor/CONTEXT.md` (modified)
- `crates/camel-dsl/CONTEXT.md` (modified)
- `crates/camel-core/CONTEXT.md` (modified)

**Steps:**
1. `docs/src/yaml-dsl/step-verbs/resilience.md` field table (`:102` area): add `on_missing_key` row (`fail | process`, default `fail`); update `remove_on_failure` default text (`true` under `eager`, `false` otherwise; explicit `false` under `eager` warns at load).
2. `docs/src/eip/idempotent-consumer.md`: add one paragraph on reservation semantics (missing keys fail by default; `process` opts into forwarding; empty == absent), one on eager rollback (default removes the key on child failure so retries re-run the work; explicit `false` keeps at-most-once with a load-time warning; a failed rollback fails the exchange and the next delivery retries the removal first), one on duplicate observability (duplicate exchanges carry the `camel.idempotent.duplicate` property and increment `camel.idempotent.duplicates`). Cite ADR-0099 (relative link `../adr/0099-idempotent-reservation-semantics.md`) and keep the two-source rule (claims trace to the ADR or `crates/camel-processor/CONTEXT.md`).
3. Architecture refresh (CONTEXT-MAP.md `:267-274` event-driven rule — this change is a contract change: a new DSL key and a public contract enum): update `CONTEXT-MAP.md` Contexts/Relationships entries affected by the reservation-semantics contract (the term `on_missing_key` lands in the map's Key Terms if cross-cutting, else crate-local); update `crates/camel-processor/CONTEXT.md` step contract (missing-key resolution, eager rollback default, pending/in-flight recovery, observability — the crate is the contract authority the guide cites); check and update `crates/camel-dsl/CONTEXT.md` (new DSL key) and `crates/camel-core/CONTEXT.md` (only if it documents this step's compiler surface).
4. Check the anchored example `examples/idempotent-consumer/routes.yaml` included by the guide: if the contract change does not invalidate it, leave it and note the check; if the guide text now contradicts the example's behavior, update the example minimally.
5. Load the `ste-writing` skill before editing prose; keep sentences short, active voice.

**Tests:**
- `cd docs && mdbook build` exits 0
- `cd docs && mdbook test` exits 0

**Acceptance:**
- Both pages build and describe the new defaults; no stale "default `false`" text remains for `remove_on_failure` under eager
- CONTEXT-MAP.md + touched-crate CONTEXT.md refreshed in the same change (event-driven rule); `cargo xtask lint-single-source` exits 0
- `cd docs && mdbook test` exits 0

- [x] 3.2
