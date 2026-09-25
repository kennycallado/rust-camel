# Tasks: dotryorig

## camel-processor

### Task 1.1: tower DoTryService catch-failure envelope (warn log + span) with shared test-log-capture helper

Implements the sealed e_opus ruling (bd rc-zgbqq): catch error stays
main; original error surfaced via log policy and span. Covers ADDED
requirement scenarios "original error surfaces in log and span",
"envelope is disposition-independent", and "catch and finally both
fail on the builder-service path" for the tower path.

**Files:**
- `crates/camel-processor/src/test_log_capture.rs` (new)
- `crates/camel-processor/src/lib.rs` (modified)
- `crates/camel-processor/src/error_handler.rs` (modified)
- `crates/camel-processor/src/do_try.rs` (modified)

**Steps:**
1. Step-0 verification: `rg -n 'previous_error|restoring previous|finally_error' crates/camel-processor/src/do_try.rs` — confirm no EXISTING do_try test asserts the restore-log fields or message (current tests assert only error variants). If any pin exists, update it in this task (the catch-failed flow's restore record changes fields to `catch_error`/`finally_error`).
2. Create `crates/camel-processor/src/test_log_capture.rs` as a
   `#[cfg(test)]`-gated `pub(crate)` module holding the log-capture
   helpers currently local to the `error_handler.rs` test module:
   `ensure_global_registry` and `capture_debugs_with_span_records`
   (move them; error_handler tests import from
   `crate::test_log_capture`). Register the module in `lib.rs` as
   `#[cfg(test)] mod test_log_capture;`. Keep every existing
   error_handler test green by import swap only — no behavior edits.
3. In `error_handler.rs`, change `fn record_span_error` (the private
   helper near `send_to_handler`) to `pub(crate) fn record_span_error`
   and adjust its doc comment to note it is shared with the do_try
   arms. No signature change.
4. In `do_try.rs`, in the `Err(failed)` arm of
   `DoTryService::call` (the "Catch threw" branch): after
   destructuring `(catch_ex, catch_err)`, emit the envelope BEFORE
   the `run_finally` call:
   `tracing::warn!(original_error = %original_err, catch_error = %catch_err, "do_try catch block failed; catch error supersedes original");`
   then `record_span_error(&catch_err);`. The `warn!` record is the
   unconditional surface; when a span is active it also attaches to
   that span as the event carrying `original_error`. Do NOT create a
   new span and do NOT add a second event. Keep the return value
   `Err(catch_err)` unchanged.
5. Refactor `run_finally` in `do_try.rs` so restore logging carries
   flow-accurate field names: replace its
   `Result<Exchange, CamelError>` with a private enum
   `FinallyOutcome { Completed(Exchange), NoPreviousFail(CamelError), Restore { previous: CamelError, finally_err: CamelError } }`
   (tower-local; unrelated to the identically-named enum in
   `do_try_segment.rs`). Move each log emission to the caller:
   - Ok-flow caller (`try Ok`, `previous` absent): on
     `NoPreviousFail(fin)` emit the existing
     `tracing::warn!(error = %fin, "doFinally threw")` then return
     `Err(fin)`; on `Restore` (unreachable here) return
     `Err(previous)`.
   - catch-Ok flow (`disposition` threads the original) and
     no-clause-match flow (both pass `previous = original_err`): on
     `Restore { previous, finally_err }` emit the existing
     `tracing::warn!(finally_error = %finally_err, previous_error = %previous, "doFinally threw; restoring previous exception (Camel parity)")`
     and return `Err(previous)`.
   - catch-failed flow (`previous = catch_err`): on
     `Restore { previous, finally_err }` emit
     `tracing::warn!(catch_error = %previous, finally_error = %finally_err, "doFinally threw after failed catch; restoring catch error")`
     and return `Err(previous)`.
   All four call sites keep today's returned `Ok`/`Err` values
   exactly (the existing tests
   `finally_throws_with_no_previous_error_propagates_finally_error`
   and `finally_throws_with_previous_error_restores_previous` must
   pass without edits — verified by the step-0 grep).
6. Add unit tests in the `do_try.rs` test module using
   `crate::test_log_capture::capture_debugs_with_span_records`:
   - `catch_throws_under_propagate_disposition_returns_catch_err`:
     try fails `ProcessorError("orig")`, matching catch clause with
     `disposition: Propagate` whose body fails `Io("catch-fail")` →
     call → result is `Err(Io)` (same envelope as Handled).
   - `catch_throws_logs_original_and_catch_error`: try fails
     `ProcessorError("orig-lost")`, matching Handled catch body fails
     `Io("catch-fail")` → run inside
     `capture_debugs_with_span_records` → assert a WARN record whose
     message contains "do_try catch block failed" carries fields
     `original_error` (contains "orig-lost") and `catch_error`
     (contains "catch-fail"), asserted by structured field lookup,
     not message formatting; assert result `Err(Io)`.
   - `catch_throws_marks_span_error_and_event`: same setup inside a
     `tracing::info_span!("dotry_test", error = tracing::field::Empty)`
     guard within the capture → assert the span records include the
     `error` field set to the catch error (via `record_span_error`)
     and the captured WARN event carries `original_error` while a
     span is active.
   - `catch_and_finally_throw_logs_finally_error`: try fails
     `ProcessorError("orig")`, matching Handled catch body fails
     `Io("catch-fail")`, finally body fails `Config("fin-fail")` →
     capture → result `Err(Io)` (catch error restored, pinned by the
     existing `catch_throws_and_finally_throws_restores_catch_err`);
     assert a WARN record carries `catch_error` (contains
     "catch-fail") and `finally_error` (contains "fin-fail").

**Tests:** (executable spec)
- `catch_throws_under_propagate_disposition_returns_catch_err`:
  Propagate-disposition catch that throws → call → `Err(Io)` exactly
  (not `ProcessorError`).
- `catch_throws_logs_original_and_catch_error`: Handled catch that
  throws → capture → WARN record with BOTH structured fields present
  and value-matching; command
  `cargo test -p camel-processor --lib do_try` → pass.
- `catch_throws_marks_span_error_and_event`: active span with empty
  `error` field → span record set to catch error; WARN event carries
  `original_error`.
- `catch_and_finally_throw_logs_finally_error`: catch+finally both
  throw → `Err(Io)` + WARN record with `catch_error` AND
  `finally_error` fields.
- Existing pins stay green: `catch_branch_throws_new_error_wins`,
  `catch_throws_with_finally_runs_finally_and_propagates_catch_err`,
  `catch_throws_and_finally_throws_restores_catch_err`,
  `finally_throws_with_no_previous_error_propagates_finally_error`,
  `finally_throws_with_previous_error_restores_previous`,
  `delegate_failure_emits_system_broken_log_and_span_error`
  (helper-move proof).

**Acceptance:**
- `cargo check -p camel-processor` exits 0.
- `cargo test -p camel-processor --lib` exits 0.
- `cargo fmt --check` clean; `cargo clippy -p camel-processor -- -D
  warnings` exits 0.
- `rg 'tracing::error!\(' crates/camel-processor/src/do_try.rs`
  returns nothing (envelope is `warn!`, no lint-log-levels surface).

- [x] 1.1

### Task 1.2: DoTrySegment catch-body-failure envelope (compiled-route path)

Same envelope on the OutcomePipeline arm. Covers ADDED requirement
scenarios "original error surfaces in log and span" and "envelope is
disposition-independent" for the compiled path.

**Files:**
- `crates/camel-processor/src/do_try_segment.rs` (modified)

**Steps:**
1. In `DoTrySegment`'s catch-body failure arm — the
   `PipelineOutcome::Failed(catch_err)` branch inside the catch loop
   (comment "Catch-body Failed: surface THAT error to outer route") —
   before `return PipelineOutcome::Failed(catch_err)`, emit:
   `tracing::warn!(original_error = %err, catch_error = %catch_err, "do_try catch block failed; catch error supersedes original");`
   then `crate::record_span_error(&catch_err);` (import the helper
   from `error_handler`). `err` is the try-body error already
   destructured in the enclosing `PipelineOutcome::Failed(err)` arm.
   Keep the return value and the ADR-0025 invariant #4 behavior
   (skip remaining catches, skip finally) unchanged. Do NOT create a
   span; the `warn!` record attaches to any active span.
2. Add unit tests in the `do_try_segment.rs` test module using
   `crate::test_log_capture::capture_debugs_with_span_records` and
   the existing test helper `fn seg_fail(err: CamelError) ->
   camel_api::OutcomeSegment` (defined in that test module) to build
   failing bodies:
   - `catch_body_failure_returns_catch_err_and_marks_span`: try body
     `seg_fail(ProcessorError("orig"))`, matching catch body
     `seg_fail(Io("catch-fail"))` → run inside
     `capture_debugs_with_span_records` with an active
     `tracing::info_span!("dotry_seg_test", error = tracing::field::Empty)`
     → outcome is `PipelineOutcome::Failed(Io)`; the span records
     include the `error` field set to the catch error (span marking
     works on the segment path too).
   - `catch_body_failure_emits_envelope_log`: same route inside the
     capture → assert a WARN record whose message contains "do_try
     catch block failed" carries `original_error` (contains "orig")
     and `catch_error` (contains "catch-fail") by field lookup.

**Tests:** (executable spec)
- `catch_body_failure_returns_catch_err`: catch body fails → run →
  `Failed(Io)` exactly; command
  `cargo test -p camel-processor --lib do_try_segment` → pass.
- `catch_body_failure_emits_envelope_log`: capture → WARN record with
  both structured fields value-matching.

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0 (existing segment
  tests untouched and green).
- `cargo fmt --check` clean; `cargo clippy -p camel-processor -- -D
  warnings` exits 0.

- [x] 1.2

## camel-test

### Task 1.3: e2e pins — translation route and failed compensation

Covers ADDED requirement scenarios "translation route matches the
catch error kind" (route-level clause selection + returned error) and
the failed-compensation observability claim (response is the catch
error; captured log carries the original error).

**Files:**
- `crates/camel-test/tests/do_try_test.rs` (modified)
- `crates/camel-test/tests/http_test.rs` (modified)

**Steps:**
1. Add `do_try_catch_translation_route_matches_catch_kind` as a
   COMPILED route (scenario says "compiled route"; pattern:
   `do_try_yaml_on_when_predicate_filters_catch_e2e` for the
   do_try/catch YAML shape + `steps_plus_handled_by_rejected_typed`
   for the `error_handler.on_exceptions` YAML shape). Two
   failure-injection builder routes (YAML has no inline throw):
   `direct:io-failing-step` (process fails `Io("orig-io")`) and
   `direct:translating-step` (process fails
   `ProcessorError("translated-domain")`). Then ONE YAML route
   `direct:translation` with `error_handler.on_exceptions` TWO
   clauses — `{kind: "ProcessorError", handled: true, handled_by:
   "direct:domain-shaper"}` and `{kind: "Io", handled: true,
   handled_by: "direct:io-shaper"}` — plus two shaper builder routes
   forwarding to `mock:domain-caught` / `mock:io-caught`. The route
   steps: `do_try` with steps `[to: "direct:io-failing-step"]` and
   one catch clause `{exception: ["Io"], disposition: handled, steps:
   [to: "direct:translating-step"]}`, followed by a downstream step
   `to: "mock:after-do-try"`. Start; oneshot a default
   exchange; assert: `mock:domain-caught` exchange count 1;
   `mock:io-caught` count 0; `mock:after-do-try` count 0.
   Do NOT assert the oneshot result kind here — the route-level
   handlers absorb the error by design (template test
   `do_try_propagate_reaches_route_on_exception` discards the
   result for exactly this reason). The returned-error observable
   for translation is pinned by the compensation test below, the
   unit tests of Task 1.1/1.2, and the HTTP-tier test in step 3.
   HTTP-status note: the kind→status mapping surface is
   `pipeline_error_to_reply` in `crates/components/camel-http/src/lib.rs`
   (Unauthenticated→401, ValidationError→400 with json body kind
   "validation_error", ProcessorError→500 default arm, …). Step 3
   pins the mapping end-to-end through a real HTTP round trip.
2. Add `do_try_catch_failure_compensation_route_logs_original`:
   builder route `direct:compensation` with NO route-level
   on_exception; `do_try` body process fails
   `Io("orig-io-lost")`; `do_catch_exception(&["Io"])` body process
   fails `ProcessorError("compensation-down")` (Handled); no
   downstream step. For log capture (camel-test cannot import
   camel-processor's cfg(test) module), add a local helper in this
   test file: build a `tracing_subscriber::fmt().json().with_max_level(tracing::Level::WARN).without_time()` subscriber writing to a
   shared `Arc<Mutex<Vec<u8>>>` buffer; wrap a current-thread-runtime
   `block_on` of the oneshot in `tracing::subscriber::with_default`
   (mirroring the shape of `error_handler.rs`'s
   `delegate_failure_emits_system_broken_log_and_span_error`). The
   workspace `tracing-subscriber` dep already enables the `json`
   feature (`features = ["fmt", "json", "env-filter"]` in the
   workspace Cargo.toml; crate features are additive) — no
   dependency edits. Parse each captured line as JSON and assert by
   FIELD lookup on the record whose message contains "do_try catch
   block failed": result is `Err(ProcessorError("compensation-down"))`; the record's
   `original_error` field contains "orig-io-lost" and its
   `catch_error` field contains "compensation-down".
3. Add `do_try_catch_failure_maps_http_status_from_catch_kind` in
   `crates/camel-test/tests/http_test.rs` (template:
   `http_pipeline_error_returns_500` — same `install_crypto_provider`
   + `stage_http_listener` + `HttpComponent` + reqwest round trip):
   builder route `http://127.0.0.1:{port}/translate` whose `do_try`
   body process fails `Unauthenticated("orig-auth")` (kind maps to
   401 via `pipeline_error_to_reply`), matching catch clause
   `do_catch_exception(&["Unauthenticated"])` whose body process
   fails `ValidationError("translated-validation")` (kind maps to
   400 with json body kind "validation_error"), Handled disposition,
   no route-level handler. GET the endpoint with reqwest → assert
   response status is 400 (derived from the CATCH error kind) and
   NOT 401 (the original kind); assert the response body text
   contains "validation_error". This pins scenario 1's
   "mapped HTTP status derives from the catch error kind" through
   the real mapping surface.
4. Use `acquire_deadline` (never `tokio::time::sleep`) for context
   and endpoint readiness waits, matching the files' existing style
   (`lint-test-sleep` hygiene); where the template tests use
   `tokio::time::sleep`, prefer `acquire_deadline` in the new tests.

**Tests:** (executable spec)
- `do_try_catch_translation_route_matches_catch_kind`: compiled YAML
  route with competing route-level clauses (ProcessorError →
  domain-caught, Io → io-caught) and a translating catch body →
  oneshot → domain mock 1; Io mock 0; no downstream exchanges;
  command `cargo test -p camel-test --test do_try_test` → pass
  (plus `cargo check -p camel-test --features integration-tests
  --tests` for the CI-only surface).
- `do_try_catch_failure_compensation_route_logs_original`: oneshot
  under JSON-capture → `Err(ProcessorError("compensation-down"))`
  AND WARN record fields `original_error`~"orig-io-lost",
  `catch_error`~"compensation-down" (field lookup on parsed JSON
  records).
- `do_try_catch_failure_maps_http_status_from_catch_kind`: HTTP
  round trip (original Unauthenticated→401, catch
  ValidationError→400) → response status 400 (not 401) + body
  contains "validation_error"; command
  `cargo test -p camel-test --test http_test do_try_catch_failure`
  → pass.

**Acceptance:**
- `cargo test -p camel-test --test do_try_test` exits 0.
- `cargo test -p camel-test --test http_test do_try_catch_failure`
  exits 0.
- `cargo check -p camel-test --features integration-tests --tests`
  exits 0.
- `cargo fmt --check` clean; `cargo clippy -p camel-test --tests --
  -D warnings` exits 0.

- [x] 1.3

## docs

### Task 1.4: document the catch-block failure envelope

**Files:**
- `docs/src/concepts/error-handling.md` (modified)
- `crates/camel-processor/src/do_try.rs` (modified)

**Steps:**
1. In `docs/src/concepts/error-handling.md`, extend the "doTry
   blocks" section (after the disposition explanation) with a short
   paragraph: when a catch body itself fails, the catch error is the
   main error in every disposition — kind matching and HTTP status
   mapping use it, so exception translation works; the original error
   is not lost: a `warn` record ("do_try catch block failed; catch
   error supersedes original") carries `original_error` and
   `catch_error`, and an active span records both. Contrast in one
   sentence with the `handled_by` delegate rule — a failing delegate
   propagates the ORIGINAL error as the main error because the
   delegate is infrastructure, not route code (source: the
   error-handler spec requirement "delegate failure propagates the
   original error", amended by this change).
2. In `crates/camel-processor/src/do_try.rs`, extend the module doc
   comment (§ Stop semantics header block) with two lines citing the
   catch-block failure envelope (catch error main; original in log
   policy + span; see the error-handler spec and bd rc-zgbqq).

**Tests:** (no runtime tests — docs only)
- `doc-build`: `RUSTDOCFLAGS="-D warnings" cargo doc -p
  camel-processor --no-deps` exits 0 (intra-doc links, if any, point
  at public items only).

**Acceptance:**
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core
  -p camel-builder -p camel-dsl -p camel-endpoint -p
  camel-processor --no-deps` exits 0 (mission doc-build set plus the
  touched crate).
- The new paragraph uses STE prose (short sentences, no "simply",
  active voice).

- [x] 1.4
