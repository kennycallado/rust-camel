# Tasks: handledby

Single-phase change (see design.md — task order encodes dependencies:
runtime contract first, then DSL layout, then fixtures/docs/schema).

## camel-api

### Task 1.1: Add typed ConfigValidationError variant for the steps/handled_by conflict

**Files:**
- `crates/camel-api/src/error.rs` (modified)

**Steps:**
1. Read `crates/camel-api/src/error.rs` and study the existing
   `ConfigValidationError` variants (naming + `#[error(...)]` style).
2. Add variant `OnExceptionStepsHandledByConflict` to
   `ConfigValidationError` with error text
   `"on_exceptions clause cannot set both steps and handled_by (delegation is exclusive)"`.
   Follow the existing variant style (unit-like or fieldless — match the
   pattern used by sibling conflict variants such as the loop count/while
   conflicts).
3. Keep `#[non_exhaustive]` on the enum as-is. Do not add a predicate
   function unless one already exists for sibling variants.

**Tests:** (executable spec)
- `config_validation_error_on_exception_conflict_display`:
  setup: variant exists → action: format it with `{}` via Display →
  assert: rendered text contains both `steps` and `handled_by`
  and the word `exclusive`. command:
  `cargo test -p camel-api --lib config_validation_error_on_exception_conflict`
  expected: fails before the variant exists (compile error), passes after.

**Acceptance:**
- `cargo test -p camel-api --lib` exits 0.
- `cargo clippy -p camel-api -- -D warnings` exits 0.

- [x] 1.1

## camel-processor

### Task 2.1: Delegate-failure contract — send_to_handler returns Err, all callers map to original-error propagation

**Files:**
- `crates/camel-processor/src/error_handler.rs` (modified)

**Steps:**
1. Read `crates/camel-processor/src/error_handler.rs` fully — map every
   `send_to_handler` call site (6 total): L47 (top-level helper), L329
   (`handle_step` match; its Err arm at ~L344 is marked "Dead code by
   construction" today), L417 (`handle_boundary` match; Err arm ~L431
   also "dead code"), L621 (retry-exhausted forward), L636 (no-retry
   policy-handler forward), L641 (no-match DLC forward). NOTE: the
   inline on_steps Err arm (~L318-321, named in the ruling) falls
   through to the L329 match — no separate call site, but verify its
   fall-through still lands on the new mapping.
2. Change `send_to_handler` (~L647): in the `Some(mut prod)` arm,
   `prod.ready()` Err and `svc.call(...)` Err now return
   `Err(delegate_error)`. Each failure logs log-policy system-broken with
   BOTH errors structured (use the structured-field
   `tracing::error!` macro style already present in this file — original
   error from `exchange.error` plus the delegate error) and records the
   delegate error on the current span (follow the span-error pattern
   already used in this crate; if no pattern exists, use
   `tracing::Span::current().record("error", ...)` guarded by
   `tracing::Span::current().is_recording()`). The `None` producer arm is
   UNCHANGED (log-only semantic returns the exchange).
3. Step-path callers — delegate Err now maps to
   `Ok(StepDisposition::Propagate(original_error))` where
   `original_error` is the ORIGINAL step error in scope at that site
   (NOT the delegate error):
   - `handle_step` match Err arm (~L344) — remove the "Dead code by
     construction" comment, return Propagate(original).
   - retry-exhausted site (~L621): the current code
     `return send_to_handler(original, handler).await;` — wrap: on Ok
     keep existing disposition mapping, on Err(delegate) produce the
     Propagate(original-error) outcome for that enclosing function's
     return type. IMPORTANT: the exhausted exchange carries redelivery
     headers and the FINAL retry error — per ruling the retry-exhausted
     error IS the current original (the last failure); propagate that.
   - no-retry policy-handler forward (~L636, `send_to_handler(ex,
     handler)`) and no-match DLC forward (~L641, `send_to_handler(ex,
     dlc)`): same mapping — Err(delegate) yields the original-error
     propagation outcome for each enclosing return type.
   - helper at ~L47: read its contract (who calls it, what its return
     type means); align it with the new Err-returning send_to_handler so
     a failed delegate never surfaces as a successful exchange there
     either.
4. Boundary caller — `handle_boundary` match (~L417): its Err arm
   (~L431, currently "Dead code by construction") returns
   `Err(original_error)` (the ORIGINAL boundary error, not the delegate
   error). `handle_boundary` returns `Result<Exchange, CamelError>` — no
   StepDisposition here.
5. Verify the pipeline outcome: `StepDisposition::Propagate(err)` and
   boundary `Err(err)` must reach the pipeline as
   `PipelineOutcome::Failed(err)` with the ORIGINAL error kind. Check
   `SequentialPipeline`/`TracedPipeline` mapping (it already maps
   Propagate → Failed) — do not change it; add a pinning test.
6. Run `cargo fmt` and `cargo clippy -p camel-processor -- -D warnings`
   before reporting.

**Tests:** (executable spec — add to the existing `mod tests` in this file; reuse its test doubles for failing/ready handlers)
- `delegate_call_failure_with_handled_propagates_original`:
  setup: policy matching any error, disposition Handled, handler
  producer whose `call` returns Err → action: run the failing step
  through the handler under test → assert: result is
  `Ok(StepDisposition::Propagate(e))` where `e` equals the ORIGINAL step
  error (not the delegate error). command:
  `cargo test -p camel-processor --lib delegate_call_failure_with_handled`
  expected: fails before the change (returns Handled exchange), passes after.
- `delegate_failure_emits_system_broken_log_and_span_error`:
  setup: same as above. The existing `capture_debugs` subscriber
  (error_handler.rs ~L2044) discards span records in `record` — EXTEND
  it (or add a sibling capturing layer) so span `on_record`/`on_follows_record`
  events are captured too; enter an active span with a declared
  `error` field around the call → assert: (i) capture contains a
  system-broken record structuring BOTH the original error and the
  delegate error; (ii) the span recorded the `error` field with the
  DELEGATE error (not the original). command:
  `cargo test -p camel-processor --lib delegate_failure_emits_system_broken`
- `retry_exhausted_then_delegate_failure_propagates_original`:
  setup: policy with retry (max_attempts 1, 1ms delays) whose step keeps
  failing AND a failing delegate call → action: run through the handler
  → assert: the L621 forward's enclosing Tower service returns
  `Err(...)` carrying the retry-exhausted (last) step error, NOT the
  delegate error and NOT an Ok exchange (the pipeline surfaces
  `Failed(exhausted_error)`). command:
  `cargo test -p camel-processor --lib retry_exhausted_then_delegate_failure`
- `on_steps_fallback_delegate_failure_propagates_original`:
  setup: policy with an on_steps pipeline that FAILS and a failing
  delegate — exercising the on_steps fallback to the handler (identify
  the concrete send_to_handler site the fallback reaches — the L47
  helper or the handle_step L329 match — by reading execute_on_steps;
  the fallback from on_steps failure must land on one of them) →
  assert: the ORIGINAL step error propagates per that site's return
  type (`Ok(StepDisposition::Propagate(original))` for the handle_step
  path, `Err(original)` for a Tower-service path) — never an Ok
  exchange masking the failure. command:
  `cargo test -p camel-processor --lib on_steps_fallback_delegate_failure`
- `no_match_dlc_delegate_failure_propagates_original`:
  setup: no policy matches the error, a DLC producer is configured and
  fails (the ~L641 no-match forward) → assert: the original step error
  propagates (never an Ok exchange masking the failure). command:
  `cargo test -p camel-processor --lib no_match_dlc_delegate_failure`
- `delegate_ready_failure_propagates_original`:
  setup: same but producer `ready()` returns Err → action/assert:
  `Ok(StepDisposition::Propagate(original))`. command:
  `cargo test -p camel-processor --lib delegate_ready_failure`
- `delegate_failure_with_continued_propagates_original`:
  setup: disposition Continued + failing delegate call → assert:
  `Ok(StepDisposition::Propagate(original))` (never Continued). command:
  `cargo test -p camel-processor --lib delegate_failure_with_continued`
- `boundary_delegate_failure_returns_original_err`:
  setup: `handle_boundary` with a Security boundary error and a failing
  delegate → assert: `Err(original_boundary_error)`. command:
  `cargo test -p camel-processor --lib boundary_delegate_failure`
- `propagate_disposition_maps_to_failed_outcome`:
  setup: pipeline with one failing step and a policy with failing
  delegate + handled:true → action: call pipeline → assert:
  `PipelineOutcome::Failed` carrying the original error kind. command:
  `cargo test -p camel-processor --lib propagate_disposition_maps_to_failed`
  (place in the pipeline test module if that is where step-disposition
  consumption is tested — follow existing test placement conventions).
- `tap_policy_without_handled_propagates`:
  setup: policy with delegate + default disposition (Propagate, no
  handled/continued) + delegate that SUCCEEDS → assert:
  `Ok(StepDisposition::Propagate(original))` and the delegate received
  the exchange (side-effect fired). command:
  `cargo test -p camel-processor --lib tap_policy_without_handled`

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0 (all new + existing tests).
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- No caller of `send_to_handler` treats a delegate Err as success
  (`rg -n 'send_to_handler' crates/camel-processor/src/error_handler.rs`
  shows every site either maps Err or propagates it as original error).

- [x] 2.1

## camel-dsl

### Task 3.1: Move handled_by to clause level — model, serde surface (route_ast), converters, compile wiring, typed conflict, camel-dsl fixtures

**Files:**
- `crates/camel-dsl/src/model.rs` (modified)
- `crates/camel-dsl/src/route_ast.rs` (modified — THE serde surface:
  `RouteDslOnException` / `RouteDslRedeliveryPolicy` ~L401-436, shared
  by YAML and JSON through `route_dsl_to_declarative_route`)
- `crates/camel-dsl/src/yaml.rs` (modified — converter mapping ~L460-500
  + fixture migrations)
- `crates/camel-dsl/src/json.rs` (modified — converter mapping + fixture
  migrations)
- `crates/camel-dsl/src/compile.rs` (modified)

**Steps:**
1. `model.rs`: add `pub handled_by: Option<String>` to
   `DeclarativeOnException`; REMOVE `pub handled_by: Option<String>`
   from `DeclarativeRedeliveryPolicy`.
2. `route_ast.rs`: apply the same move — `RouteDslOnException` gains
   `handled_by` (Option, absent-tolerant like sibling optional fields);
   `RouteDslRedeliveryPolicy` loses it. Both structs ALREADY carry
   `#[serde(deny_unknown_fields)]` — do NOT add it again; the legacy
   `retry: {handled_by}` layout becomes a parse error automatically
   (spec scenario (g) YAML+JSON, both formats share this struct).
3. `yaml.rs` + `json.rs` converter mappings (the
   `route_dsl_to_declarative_route` mapping, yaml ~L460-500 and the json
   equivalent): map `clause.handled_by` →
   `DeclarativeOnException.handled_by`; drop the `handled_by` line from
   the retry mapping (yaml ~L476 and ~L494, json equivalent).
4. `compile.rs` clause loop (~L805): move the `builder.handled_by(uri)`
   wiring OUT of the `if let Some(retry) = clause.retry` block so it
   reads the new clause field unconditionally after the retry wiring:
   `if let Some(uri) = clause.handled_by { builder = builder.handled_by(uri); }`.
5. `compile.rs` top-level `error_handler.retry` path (~L879): DELETE the
   `if let Some(uri) = retry.handled_by` block (field no longer exists;
   `dead_letter_channel` is the catch-all).
6. `compile.rs` `validate_error_handler` (~L2232): for each clause, when
   `!clause.steps.is_empty() && clause.handled_by.is_some()` return the
   typed rejection from Task 1.1: return
   `CamelError::ConfigValidation(ConfigValidationError::OnExceptionStepsHandledByConflict)`
   (verified: `CamelError::ConfigValidation` exists, error.rs ~L224,
   with a `From<ConfigValidationError>` impl ~L421 — use it; do NOT use
   `CamelError::Config(String)` message matching).
7. `validate_redelivery_policy` stays unchanged (`max_attempts == 0`
   still rejected).
8. Migrate ALL camel-dsl fixtures that put `handled_by` inside `retry:`
   to the clause level — sweep with
   `rg -n 'handled_by' crates/camel-dsl/src/` — known sites:
   compile.rs tests (~L2360 and ~L2960 regions), yaml.rs parse test
   (~L2493, asserting `clauses[0].retry.handled_by` — move the
   assertion to `clauses[0].handled_by`),
   `test_parse_yaml_supports_all_declarative_step_kinds`
   (yaml.rs ~L3364, route-level `error_handler.retry` fixture carrying
   `handled_by: "log:handled"` — drop the key from that fixture; the
   route-level retry has no handler field anymore), json.rs
   `test_error_handler_with_handled_by` (~L202-237 — move assertion to
   `clauses[0].handled_by`).
9. Run `cargo fmt` + `cargo clippy -p camel-dsl -- -D warnings` +
   `cargo test -p camel-dsl`.

**Tests:** (executable spec — in compile.rs `mod tests`; the parse-level
hard-error behavior is pinned by Task 4.1)
- `compile_clause_level_handled_by_without_retry`:
  setup: DeclarativeOnException with kind Io, handled:true,
  handled_by:"log:io", retry:None → action: compile the error handler →
  assert: built policy has `handled_by == Some("log:io")` and
  `retry.is_none()`. command:
  `cargo test -p camel-dsl --lib compile_clause_level_handled_by_without_retry`
- `compile_retry_composes_with_handled_by`:
  setup: clause with retry max_attempts:2 AND handled_by → assert:
  built policy has BOTH `retry.is_some()` and
  `handled_by == Some(uri)`. command:
  `cargo test -p camel-dsl --lib compile_retry_composes_with_handled_by`
- `compile_steps_plus_handled_by_is_typed_rejection`:
  setup: clause with non-empty steps AND handled_by → action: compile →
  assert: Err carries `ConfigValidationError::OnExceptionStepsHandledByConflict`
  (match the `CamelError::ConfigValidation` payload discriminant, not
  the message string). command:
  `cargo test -p camel-dsl --lib compile_steps_plus_handled_by`
- existing migrated fixtures still pass under the new layout. command:
  `cargo test -p camel-dsl`.

**Acceptance:**
- `cargo test -p camel-dsl` exits 0 (lib + tests/).
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.
- `rg -n 'retry.*handled_by|handled_by.*retry' crates/camel-dsl/src/compile.rs`
  shows no remaining reads of a handled_by field from a retry struct.
- KNOWN TRANSIENT (documented, not a defect): camel-test suites with
  legacy-layout YAML fixtures turn RED at this task (route_ast's
  deny_unknown_fields now rejects them at parse). They are migrated in
  Task 5.1. This task's gate is camel-dsl only — do NOT run camel-test
  gates here.

- [x] 3.1

### Task 4.1: Parse-level hard-error tests — legacy layout rejection pinned in yaml.rs + json.rs

**Files:**
- `crates/camel-dsl/src/yaml.rs` (modified — tests only)
- `crates/camel-dsl/src/json.rs` (modified — tests only)

**Steps:**
1. Task 3.1 already moved the serde surface (`route_ast.rs`
   `RouteDslOnException`/`RouteDslRedeliveryPolicy`, shared by both
   formats, already `deny_unknown_fields`). THIS task only pins the
   resulting parse behavior with tests in both format entry points.
   Do NOT touch serde structs or converter mappings here.
2. `yaml.rs` tests module: add
   `test_yaml_legacy_retry_handled_by_is_hard_error` (clause-level
   legacy), `test_yaml_clause_level_handled_by_parses` (new layout),
   `test_yaml_top_level_retry_handled_by_rejected` (route-level retry).
3. `json.rs` tests module: add
   `test_json_legacy_retry_handled_by_is_hard_error`.
4. Run `cargo fmt` + `cargo clippy -p camel-dsl -- -D warnings` +
   `cargo test -p camel-dsl`.

**Tests:** (executable spec)
- `test_yaml_legacy_retry_handled_by_is_hard_error` (yaml.rs):
  setup: YAML route with
  `on_exceptions: [{kind: "Io", retry: {max_attempts: 1, handled_by: "log:io"}}]`
  → action: `parse_yaml_to_declarative` → assert: Err whose text
  contains `handled_by` and `unknown field`. command:
  `cargo test -p camel-dsl --lib test_yaml_legacy_retry_handled_by`
- `test_json_legacy_retry_handled_by_is_hard_error` (json.rs):
  setup: JSON equivalent with `"retry": {"max_attempts": 1, "handled_by": "log:io"}`
  → assert: Err naming `handled_by` as unknown field. command:
  `cargo test -p camel-dsl --lib test_json_legacy_retry_handled_by`
- `test_yaml_clause_level_handled_by_parses`:
  setup: YAML clause with top-level `handled_by: "log:io"` and NO retry
  → assert: parsed clause has
  `handled_by == Some("log:io")`, `retry.is_none()`. command:
  `cargo test -p camel-dsl --lib test_yaml_clause_level_handled_by`
- `test_yaml_top_level_retry_handled_by_rejected`:
  setup: route-level `error_handler: {retry: {max_attempts: 1,
  handled_by: "log:x"}}` → assert: Err naming `handled_by` unknown field
  (top-level retry equally strict). command:
  `cargo test -p camel-dsl --lib test_yaml_top_level_retry_handled_by_rejected`

**Acceptance:**
- `cargo test -p camel-dsl` exits 0.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.

- [x] 4.1

## camel-test

### Task 5.1: Scenario tests (a)-(h) end-to-end + fixture migration

**Files:**
- `crates/camel-test/tests/on_exceptions_wildcard_test.rs` (modified)
- `crates/camel-test/tests/do_try_test.rs` (modified — only fixtures
  using `retry: {handled_by}`; do_try semantics untouched)
- `crates/camel-test/tests/integration_test.rs` (modified)
- `crates/camel-test/tests/on_exceptions_handled_by_test.rs` (new)

**Steps:**
1. Sweep `rg -n 'handled_by' crates/camel-test/tests/` — migrate every
   fixture that puts a `handled_by` key inside the `retry:` mapping to
   the clause-level layout. This RESTORES the camel-test suites that
   Task 3.1's route_ast field move turned into parse errors (the
   documented transient).
2. Create `on_exceptions_handled_by_test.rs` with the eight ruling
   scenarios, using the existing camel-test harness style from
   `on_exceptions_wildcard_test.rs` (mock/direct handler routes, failing
   steps). A failing delegate = handler route whose step returns Err
   (reuse the pattern the suite uses for failing endpoints; if no
   existing failing-delegate pattern exists, use a mock endpoint
   configured to fail).
3. Scenario (a): clause `{kind: "Io", handled: true, handled_by:
   "direct:shaper"}` no retry → step body records invocations (counter
   bean or mock endpoint hit count) → assert exactly 1 execution,
   exchange reaching shaper has NO `CamelRedelivered` header, pipeline
   Completed with shaper output.
4. Scenario (b): `{kind: "Io", handled: true, retry: {max_attempts: 2,
   initial_delay_ms: 1, multiplier: 1.0, max_delay_ms: 1}, handled_by:
   "direct:shaper"}` → assert 3 total step executions, shaper invoked
   once, delegated exchange headers `CamelRedelivered == true`,
   `CamelRedeliveryCounter == 2`, `CamelRedeliveryMaxCounter == 2`.
   (Use minimal delays — the lint-test-sleep policy forbids real sleeps;
   backoff of 1ms is config, not test sleep.)
5. Scenario (c): shaper route FAILS → clause handled:true → assert
   final outcome Failed with the ORIGINAL Io error kind (assert the
   error variant, not the message).
6. Scenario (d): same as (c) with continued:true instead of handled:true
   → assert Failed with original error kind.
7. Scenario (e): security policy (or circuit breaker) denial routed to a
   failing handled_by delegate → assert Failed with the ORIGINAL
   boundary error kind (Unauthenticated/security denial variant).
8. Scenario (f): route YAML with clause carrying both steps and
   handled_by → assert load/compile fails with
   `ConfigValidationError::OnExceptionStepsHandledByConflict` reachable
   through the error (match discriminant or downcast per the existing
   typed-error assertion pattern in camel-test).
9. Scenario (g): YAML + JSON route strings with legacy
   `retry: {handled_by}` → assert load fails naming unknown field
   `handled_by`.
10. Scenario (h): clause `{kind: "Io", handled_by: "direct:audit"}` (no
    handled/continued) → assert audit route received the exchange AND
    outcome Failed with original Io error kind (tap).
11. Update wildcard test fixtures (they spell the old layout — see the
    MODIFIED dsl spec scenarios for the new canonical text) and run the
    full camel-test set for these files.

**Tests:** (executable spec)
- `zero_retry_delegation_runs_once_then_delegates` — scenario (a);
  command:
  `cargo test -p camel-test --test on_exceptions_handled_by_test zero_retry`
- `retry_composes_then_delegates_with_redelivery_headers` — scenario (b);
  command:
  `cargo test -p camel-test --test on_exceptions_handled_by_test retry_composes`
- `failed_delegate_with_handled_fails_original` — scenario (c); command:
  `cargo test -p camel-test --test on_exceptions_handled_by_test failed_delegate_with_handled`
- `failed_delegate_with_continued_fails_original` — scenario (d); command:
  `cargo test -p camel-test --test on_exceptions_handled_by_test failed_delegate_with_continued`
- `failed_delegate_at_boundary_fails_original` — scenario (e); command:
  `cargo test -p camel-test --test on_exceptions_handled_by_test failed_delegate_at_boundary`
- `steps_plus_handled_by_rejected_typed` — scenario (f); command:
  `cargo test -p camel-test --test on_exceptions_handled_by_test steps_plus_handled_by`
- `legacy_retry_handled_by_hard_error_yaml_and_json` — scenario (g); command:
  `cargo test -p camel-test --test on_exceptions_handled_by_test legacy_retry_handled_by`
- `handled_by_without_handled_is_tap` — scenario (h); command:
  `cargo test -p camel-test --test on_exceptions_handled_by_test handled_by_without_handled`
- Migration suites stay green:
  `cargo test -p camel-test --test on_exceptions_wildcard_test`,
  `cargo test -p camel-test --test do_try_test`,
  `cargo test -p camel-test --test integration_test`

**Acceptance:**
- All eight scenario tests pass.
- The three migrated suites pass unchanged in behavior (only layout
  changed in fixtures).
- `cargo clippy -p camel-test -- -D warnings` exits 0 (or the repo's
  clippy invocation covering camel-test).

- [x] 5.1

## docs

### Task 6.1: ADR-0019 amendment + docs/examples alignment

**Files:**
- `docs/adr/0019-error-disposition-pipeline-recovery.md` (modified)
- `docs/src/concepts/error-handling.md` (modified)
- `docs/src/yaml-dsl/step-verbs.md` (modified — only if it documents the
  retry/handled_by layout; sweep first)
- `examples/error-handling/src/main.rs` (modified)

**Steps:**
1. ADR-0019: append an amendment section "Amendment (2026-09-24):
   delegate failure and clause-level handled_by" recording: (i)
   `handled_by` is a clause-level disposition field; retry composes
   (retry first, delegate after exhaustion — Apache Camel onException
   parity); (ii) delegate failure maps to original-error propagation in
   every disposition, `Err(original)` at `handle_boundary`, pipeline
   outcome `Failed(original kind)`, never `Completed`; delegate error
   surfaced via system-broken log (both errors) + span error; (iii) the
   do_try difference: a failing catch propagates the CATCH error and
   loses the original — intentional divergence, unchanged (rc-zgbqq);
   (iv) typed steps+handled_by rejection; legacy `retry:{handled_by}` is
   a hard load error.
2. `docs/src/concepts/error-handling.md` and
   `docs/src/yaml-dsl/step-verbs.md`: move every `handled_by` example
   from inside `retry:` to the clause level (sweep
   `rg -n 'handled_by' docs/src/`); document the hard error for the old
   layout and the tap semantics.
3. `examples/error-handling/src/main.rs`: migrate any
   `retry: {handled_by}` usage to clause-level and keep the example
   building (`cargo check -p error-handling` from examples dir — or the
   workspace example target name found via
   `cargo metadata` / the examples README; verify with
   `cargo check --example error-handling` if it is a workspace example).
4. Keep prose in English, ASD-STE-flavored plain sentences, short.

**Tests:** (executable spec)
- Not applicable (docs) — verification is the sweep returning no stale
  layouts: `rg -n 'retry:\s*$' docs/src examples/error-handling -A3 |
  rg 'handled_by'` produces no hits outside archived/historical ADR
  sections that quote the OLD layout deliberately (the ADR amendment
  may show the old layout inline as the rejected form).

**Acceptance:**
- `rg -n 'handled_by' docs/src/ examples/error-handling/` shows only
  clause-level usages.
- ADR amendment present with all four points.

- [x] 6.1

## schema

### Task 7.1: Regenerate schema assets and verify schema-check

**Files:**
- `schemas/dsl/route-schema.json` (modified — generated)
- `schemas/ts/RouteDslRedeliveryPolicy.ts` (modified — generated)
- any sibling generated ts files for the clause model (generated)

**Steps:**
1. Find the schema generation command: `cargo xtask schema --help`
   (the check flag is `--check`; identify the generate/default mode).
2. Run schema GENERATION from the worktree root; confirm the emitted
   assets move `handled_by` from the redelivery policy object to the
  on_exceptions clause object (both JSON schema and TS types).
3. Run `cargo xtask schema --check` — must exit 0.
4. If generation output includes the route AST or REST models that
   mirror the clause, confirm they reflect the same move (no stale
   `handled_by` inside any redelivery/policy type:
   `rg -n 'handled_by' schemas/` shows it only under the clause).

**Tests:** (executable spec)
- `cargo xtask schema --check` exits 0 after generation.
- `rg -n 'handled_by' schemas/` — every hit is inside the
  on-exception clause definition, none inside a redelivery-policy
  definition.

**Acceptance:**
- `cargo xtask schema --check` exit 0.
- Schema diff reviewed: only the handled_by move (no unrelated churn).

- [x] 7.1
