# Tasks: itest-partner-faults-and-asserts

## Phase 1: Script behaviors (delay, fault, times)

### Task 1.1: Partners grammar for times, delay, and fault

Extend the `partners:` entry grammar with `times`, `delay`, and `fault`,
enforced at load with errors naming the partner key and field.

- **Files:**
  - `crates/camel-integration-test/src/document.rs` (modified)
- **Steps:**
  1. Add `#[non_exhaustive] pub enum PartnerFault { Close }` (derive
     `Debug, Clone, PartialEq, Eq`, matching adjacent model enums; the
     attribute follows ADR-0049 and the crate CONTEXT.md public-enum
     policy) next to `PartnerScript`.
  2. Extend `PartnerScript` with `pub times: Option<u32>`, `pub delay:
     Option<Duration>`, and `pub fault: Option<PartnerFault>`, and make
     `response` `Option<PartnerScriptResponse>` (the `response`/`fault`
     XOR requires it).
  3. Extend `RawPartnerScript` with `times: Option<u64>`, `delay:
     Option<String>`, `fault: Option<String>` under the existing
     `deny_unknown_fields`, and make its `response`
     `Option<RawPartnerScriptResponse>`.
  4. In the `partners:` conversion (the block that today checks
     `response.status` range), add: `times` below 1 fails with a message
     naming the partner key and `times`; `delay` parses via
     `humantime::parse_duration` and on failure names the partner key,
     `delay`, and the underlying error; `fault` must equal `"close"`
     (case-sensitive) or the error names the partner key, `fault`, and the
     value; `response` and `fault` both present or both absent fails with
     a message naming the partner key and both fields. A raw `times`
     above `u32::MAX` fails the same way via `u32::try_from` (no
     `unwrap`). All use the existing partners `DocError` channel.
  5. Update `partner_scripts_for` for the `Option` response only: map
     `response.as_ref()` with the existing status/header/body logic and
     serve fault entries (no `response`) with placeholder status 200,
     empty headers, empty body, so the crate compiles and the http
     clippy gate of this task passes. Task 1.2 extends this mapping
     with `times`, `delay`, and `fault`.
- **Tests** (in the document parse-test module where the existing partners
  grammar tests live):
  - `times_zero_is_load_error` — setup: doc with `times: 0`; action:
    parse; assert: error message contains the partner key and `times`.
  - `both_response_and_fault_is_load_error` — both fields present;
    assert message names key, `response`, `fault`.
  - `neither_response_nor_fault_is_load_error` — matchers only; assert
    message names key and both missing fields.
  - `unknown_fault_name_is_load_error` — `fault: reset`; assert message
    names key, `fault`, and `reset`.
  - `bad_delay_is_load_error` — `delay: 500xyz`; assert message names
    key, `delay`, and the humantime error text.
  - `times_delay_fault_parse` — one entry with `times: 2`, `delay:
    300ms`, `fault: close` (and a second entry with plain `response`);
    assert parsed `PartnerScript` fields equal the input.
  - command: cargo test -p camel-integration-test --lib`
  - expected: fail before implementation (fields unknown to serde),
    pass after.
- **Acceptance:**
  - `cargo test -p camel-integration-test --lib` passes, existing
    partners grammar tests unchanged.
  - `cargo clippy -p camel-integration-test --features http --all-targets
    -- -D warnings` exits 0.
- [x] 1.1

### Task 1.2: Serve semantics — counter consume, delay, fault abort

Serve unspent entries by remaining count, hold for `delay` outside the
lock, and abort the connection for `fault: close` after recording.

- **Files:**
  - `crates/camel-integration-test/src/adapters/http.rs` (modified)
- **Steps:**
  1. Extend wire `ScriptedResponse` with `pub times: u32`, `pub delay:
     Option<Duration>`, and `pub fault: Option<PartnerFault>` (import
     from `crate::document`), replacing the derived `Default` with a
     manual impl that yields `status: 200` and `times: 1` (a derived
     `Default` would produce zeros); assert this in a unit test.
  2. Extend `partner_scripts_for` in `document.rs` to carry the new
     fields from each parsed `PartnerScript` into the wire
     `ScriptedResponse`: `times` maps as `script.times.unwrap_or(1)`
     (serve-once default; a derived `Default` would yield 0), and a
     fault entry (no `response`) maps with placeholder status 200,
     empty headers, empty body — the serve path checks `fault` first,
     so placeholders never reach the wire. The CLI delegate path needs
     no change.
  3. Replace the removal consume in the serve path: under the scripted
     lock, find the position of the first entry whose matchers match AND
     whose remaining count exceeds zero; decrement its `times` field,
     removing the entry when it reaches zero; clone the entry's response
     data (status, headers, body, delay, fault) out; drop the lock before
     any await.
  4. After the lock: if `delay` is set, `tokio::time::sleep` it; then if
     `fault == Some(PartnerFault::Close)`, abort the connection instead
     of building a response (return a service error from the hyper
     handler so hyper drops the connection with no HTTP response bytes).
  5. Leave the record-then-script order and the permissive fallback
     untouched; the permissive path stays non-consuming.
- **Tests** (adapter-level, in the existing http test module or a sibling
  test file, using `HttpPartner::start` and raw HTTP client calls):
  - `times_two_serves_two_then_falls_through` — setup: entries
    `[{times: 2, status: 201}, {status: 200, body: "fallback"}]` both
    matching any request; action: three requests; assert: statuses
    `201, 201, 200` and third body `fallback`.
  - `delay_holds_response` — entry `delay: 300ms`, status 200; action:
    one request; assert: elapsed >= 300ms, status 200.
  - `fault_close_yields_transport_error_and_records` — entry
    `fault: close`; action: one request via a client that surfaces
    connection-level errors; assert: request fails without an HTTP
    status, and `recorded_requests()` on the partner's recorder holds
    exactly one entry.
  - `delay_before_fault` — entry `delay: 200ms` + `fault: close`;
    assert: transport error and elapsed >= 200ms.
  - `matched_entries_without_times_still_serve_once` — regression: two
    identical plain entries; two requests; assert first gets entry one's
    body, second gets entry two's (pre-change behavior preserved).
  - command: cargo test -p camel-integration-test --features http`
  - expected: fail before implementation (fields absent on wire
    struct), pass after.
- **Acceptance:**
  - All new tests pass; existing scripting tests pass unchanged.
  - `cargo clippy -p camel-integration-test --features http --all-targets
    -- -D warnings` exits 0; `cargo fmt --check` clean.
  - No `await` while the scripted lock is held (code-reviewed in 1.2
    review).
- [x] 1.2

### Task 1.3: Scripting e2e through the scenario runner

Prove grammar-to-wire wiring with whole-document scenario runs.

- **Files:**
  - `crates/camel-integration-test/tests/http_partner_scripting_test.rs`
    (modified)
- **Steps:**
  1. Add documents (following the file's existing doc-fixture pattern)
     exercising each new behavior end to end.
  2. Delay document: one delayed 200 entry with body; scenario sends,
     receives, and validates status plus body (timing is proven at
     adapter level in Task 1.2; no elapsed assertions here).
  3. Fault document: entry with `fault: close`; scenario sends then
     receives (the parked roundtrip surfaces the fault); assert
     `run_scenario_document` returns the transport-class
     `ScenarioFailure::ActionTransport` at the receive action, and the
     partner recorder holds the faulted request (implementation
     correction: `ClientLane::launch` returns after connect, so the
     fault is observed by the receive, not the send).
  4. Delay-before-fault document: `delay: 100ms` + `fault: close`;
     assert the same receive-surfaced transport-class failure.
  5. Times document: entry `times: 2` (status 201, body `A`) then a
     fallback entry (status 200, body `B`); scenario sends three times,
     receiving and validating `201/A`, `201/A`, `200/B` in order.
- **Tests:**
  - `delay_response_serves_e2e` — assert receive validates the scripted
    status and body.
  - `fault_close_fails_receive_e2e` — assert `ActionTransport` at the
    receive action plus recorder length 1.
  - `delay_before_fault_fails_receive_e2e` — assert `ActionTransport`
    at the receive action.
  - `times_two_then_fallback_e2e` — assert the three ordered
    receive validations.
  - command: cargo test -p camel-integration-test --features http
    --test http_partner_scripting_test`
  - expected: fail before Task 1.1/1.2 land, pass after.
- **Acceptance:**
  - The four spec scenarios `delay holds the response before serving`,
    `fault close breaks the connection without a response`, `times
    serves N matching requests then spends the entry`, and `delay
    applies before a fault` are each covered by a green e2e.
  - `cargo clippy -p camel-integration-test --features http --all-targets
    -- -D warnings` exits 0.
- [x] 1.3

### Task 1.4: README grammar rows for the new fields

- **Files:**
  - `crates/camel-integration-test/README.md` (modified)
- **Steps:**
  1. Extend the `partners:` grammar rows with `times` (integer >= 1,
     default 1), `delay` (humantime string), and `fault` (`close`),
     and state the `response`/`fault` XOR.
  2. Add a two-entry example snippet: one `times` + `delay` + `response`
     entry and one `fault: close` entry (mirroring the Task 1.3
     documents).
  3. State that an absent `times` keeps the shipped serve-once behavior.
- **Tests:**
  - name: README keyword check; setup: README updated; action: grep the
    file; assert: `times`, `delay`, `fault`, and `close` appear in the
    partners section; command: `grep -c 'times\|delay\|fault'
    crates/camel-integration-test/README.md`; expected: nonzero.
- **Acceptance:**
  - `cargo xtask lint-context-citations` exits 0.
  - No behavior claims beyond what Tasks 1.1-1.3 proved.

## Phase 2: Partner verification (count asserts)
- [x] 1.4

### Task 2.1: Validate grammar — partner target, count expectation, deadline

- **Files:**
  - `crates/camel-integration-test/src/document.rs` (modified)
  - `crates/camel-integration-test/src/runner.rs` (modified)
- **Steps:**
  1. Add `Partner` variant to `ScenarioTarget`: `Partner(EndpointRef)`
     (the enum is already `non_exhaustive`).
  2. In `build_target`, accept key `partner` parsing `RawEndpointRef`
     via the existing `endpoint_from_raw`.
  3. Add `pub struct PartnerExpectation { pub count: u64, pub method:
     Option<String>, pub path: Option<String> }`.
  4. Introduce `#[non_exhaustive] pub enum ValidateExpectation {
     Message(Expectation), Partner(PartnerExpectation) }` (derives
     matching adjacent model enums) where `Expectation` is the existing
     message-expectation type produced by `expectation_from_value`;
     `ScenarioAction::Validate` carries `expectation:
     ValidateExpectation` (replacing the bare message expectation) and a
     new `deadline: Option<Duration>`.
  5. `RawValidate` gains `deadline: Option<String>` parsed with
     `humantime`; parse failure, or a `deadline` present when the target
     is not `Partner`, fails naming the action index and `deadline`.
  6. When the target is `Partner`, parse `expectation` as a map:
     required `count` (non-negative integer; missing, negative, or
     non-integer fails naming `count`), optional `method` and `path`
     strings; unknown keys fail naming the key. When the target is
     `LastReceived` or `Variable`, keep today's `expectation_from_value`
     path under the `Message` arm.
  7. Extend both exhaustive matches over `ScenarioTarget` so the crate
     compiles after this task alone: `bindings()` (document.rs) gains
     `Partner(_) => Vec::new()`. In `runner.rs`, `run_action`
     destructures the new `deadline` field and passes it through;
     `validate_action` now takes `&ValidateExpectation`, keeps today's
     logic under the `Message` arm verbatim, and gains a temporary
     `Partner` arm failing with `validation-mismatch: partner
     verification lands in the router task` (replaced in Task 2.2).
  8. Load cross-check: a `Partner` target URI must equal a declared
     harness `http` endpoint ref. At parse time, collect the declared
     harness endpoint URIs from the scenario's own `send`/`receive`
     endpoint refs (URI string equality) and fail naming the URI when
     no match; the CLI driver keeps its existing partners-key check
     unchanged.
- **Tests** (document parse-test module):
  - `partner_target_with_count_parses` — target partner + expectation
    `{count: 3, method: POST}`; assert fields.
  - `undeclared_partner_target_is_load_error` — URI equals no declared
    ref; assert message names the URI.
  - `deadline_on_lastreceived_is_load_error` — target `lastReceived`
    plus `deadline: 5s`; assert message names the action and `deadline`.
  - `missing_count_is_load_error` — expectation `{method: POST}`;
    assert message names `count`.
  - `negative_count_is_load_error` — `count: -1`; assert message names
    `count`.
  - `unknown_expectation_field_is_load_error` — expectation
    `{count: 1, duration: 5s}`; assert message names `duration`.
  - `unparseable_deadline_is_load_error` — `deadline: 5x`; assert
    message names `deadline`.
  - existing `lastReceived`/`variable` validate tests unchanged.
  - command: cargo test -p camel-integration-test --lib`
  - expected: fail before, pass after.
- **Acceptance:**
  - `cargo test -p camel-integration-test --lib` passes.
  - `cargo clippy -p camel-integration-test --features http --all-targets
    -- -D warnings` exits 0.
- [x] 2.1

### Task 2.2: Router snapshot, filters, and polled count assert

- **Files:**
  - `crates/camel-integration-test/src/adapters.rs` (modified)
  - `crates/camel-integration-test/src/adapters/http.rs` (modified)
  - `crates/camel-integration-test/src/runner.rs` (modified)
- **Steps:**
  1. `PartnerAdapter` gains `fn recorded_requests(&self) ->
     Vec<HttpWireRequest>` with a default returning `Vec::new()`;
     `HttpPartner` overrides it returning its recorder's snapshot
     (`HttpWireRequest` moves behind the same `http` feature gate the
     http adapter already uses; gate the trait method identically).
  2. `PartnerRouter` gains `pub fn recorded_requests(&self, key: &str)
     -> Vec<HttpWireRequest>` looking up the registered adapter by
     declared key.
  3. Add `pub(crate) fn matching_requests(requests:
     &[HttpWireRequest], method: Option<&str>, path: Option<&str>) ->
     usize` in `runner.rs`: method compares `eq_ignore_ascii_case`,
     path compares exact (path-and-query), `None` filters pass all.
  4. Make `validate_action` async and add the `Partner` branch: resolve
     the target's declared key against the router; snapshot
     `recorded_requests`; count with `matching_requests`. Without
     `deadline`: equality passes, mismatch fails with
     `validation-mismatch: action {index}: partner {uri}: expected {E},
     actual {A}` plus filter text (`method {m}`, `path {p}` when set).
     With `deadline`: poll every 100ms until equality or the deadline;
     on expiry take one final snapshot and apply the same
     equality-or-fail with the final count as actual. Update
     `run_action` to await it.
  5. Keep the `Message` arm exactly as today (it ignores `deadline`;
     the grammar already rejected that combination in Task 2.1).
- **Tests** (crate tests, http feature):
  - `matching_requests_filters_method_case_insensitive_and_exact_path`
    — unit on `matching_requests`.
  - `immediate_count_passes_and_mismatch_names_counts` — fixture boot,
    one send, validate `count: 2` no deadline; assert failure message
    contains the partner URI, `expected 2`, `actual 1`.
  - `poll_passes_once_count_settles` — one send lands before run;
    a `tokio::spawn` task sleeps 300ms then sends two more requests
    directly to the partner's bound address; validate `count: 3`,
    `deadline: 5s`; assert pass (completing before the deadline proves
    early polling; no wall-clock upper bound).
  - `overshoot_never_passes` — four sends land first; validate
    `count: 3`, `deadline: 1s`; assert failure with `actual 4`.
  - `deadline_expiry_reports_final_actual` — one send; validate
    `count: 3`, `deadline: 1s`; assert failure message `actual 1` and
    elapsed >= 1s.
  - command: cargo test -p camel-integration-test --features http`
  - expected: fail before, pass after.
- **Acceptance:**
  - All pass; `cargo clippy -p camel-integration-test --features http
    --all-targets -- -D warnings` exits 0.
  - Poll loop sleeps between snapshots (no busy wait) and holds no lock
    across await.
- [x] 2.2

### Task 2.3: Verification e2e and the fault-to-healthy flagship

- **Files:**
  - `crates/camel-integration-test/tests/partner_verification_test.rs`
    (new)
  - `crates/camel-integration-test/tests/fixtures/retry-route.yaml`
    (new)
  - `crates/camel-cli/src/commands/test/scenario.rs` (modified)
- **Steps:**
  1. Whole-document e2e tests following the scripting e2e file's
     fixture pattern, covering each ADDED spec scenario.
  2. Flagship composition — a REAL retrying route over the shipped
     two-layer bindVar stack:
     - Route file `tests/fixtures/retry-route.yaml` (new): `from:
       "http://127.0.0.1:18221/order"` (pinned listener port, the
       inbound-put pattern the shipped inbound e2e tests use),
       `error_handler: {retry: {max_attempts: 1, initial_delay_ms:
       50}}` (max_attempts counts redeliveries: 1 original + 1 retry = 2 wire
       attempts), `steps: [{to: "${env:PARTNER_URL}/order"}]` — env-tier
       form (`${env:...}`), mirroring the error-handler demo in
       `examples/yaml-dsl/config/routes.yaml` lines 175-184, compiled
       by `compile_error_handler`.
     - Document: `routeFiles` points at the fixture. The partner is
       declared ONLY through action endpoint refs (the harness folds
       `bindVar: PARTNER_URL` into the layered env as `http://host:port`
       — never set `PARTNER_URL` under document `env`, it is reserved).
       Partner scripts `[{fault: close, path: /order}, {status: 200,
       body: ok}]`.
     - Scenario actions in order: (a) `send` to
       `http://127.0.0.1:18221/order` (plain string; case-c literal
       dial onto the route listener); (b) `receive` from the same
       endpoint (client-role; consumes the parked roundtrip — the
       route's final response after its retry succeeds; validate the
       healthy body); (c) `receive` from the partner ref
       (`endpoint: http://127.0.0.1:0/order, provisioning: harness,
       bindVar: PARTNER_URL`) — this both DECLARES the partner for
       binding and consumes the first (faulted) arrival from the
       partner's arrival lane; (d) `validate` `target: {partner:
       http://127.0.0.1:0/order}` with `{count: 2, path: /order}` and
       `deadline: 5s`. Attempt one faults (recorded), the route
       retries, attempt two succeeds, and the recorder proves both
       hits; the poll tolerates the retry's 50ms backoff.
  3. Driver-level load-error proof: in the test module of
     `crates/camel-cli/src/commands/test/scenario.rs`, beside the
     existing `partners_key_typo_fails_load` test, add
     `undeclared_partner_target_exits_two`: a document whose validate
     partner URI is undeclared exits 2 with `doc-validation` naming the
     URI.
- **Tests:**
  - `immediate_count_assert_e2e` — three sends then validate
    `{count: 3, method: POST}`; assert scenario completes.
  - `count_mismatch_fails_e2e` — one send, validate `count: 3`;
    assert `validation-mismatch` naming partner, expected 3, actual 1.
  - `filters_narrow_count_e2e` — two `GET` and two `POST` sends;
    validate `{count: 2, method: GET}`; assert pass.
  - `deadline_polls_until_settle_e2e` — background task lands request
    three at +300ms; validate `count: 3, deadline: 5s`; assert pass.
  - `never_settles_fails_at_deadline_e2e` — one send; validate
    `count: 3, deadline: 1s`; assert `validation-mismatch` with
    `actual 1`.
  - `route_retries_faulted_partner_then_count_e2e` — the flagship
    above: assert the scenario completes, the send's response is the
    healthy body, and validate count 2 passes.
  - driver-level: `undeclared_partner_target_exits_two` — assert exit
    code 2 and the URI in stderr.
  - command: cargo test -p camel-integration-test --features http
    --test partner_verification_test` plus the driver test invocation.
  - expected: fail before Task 2.2, pass after.
- **Acceptance:**
  - All eight ADDED spec scenarios covered by green tests (five here,
    three load-error proofs split between Task 2.1 unit tests and the
    driver-level test here).
  - `cargo test -p camel-cli --features integration-http` passes.
- [x] 2.3

### Task 2.4: Docs and runnable fault-to-healthy example

- **Files:**
  - `crates/camel-integration-test/README.md` (modified)
  - `examples/integration-testing/partner-retry-route.test.yaml` (new; .test.yaml suffix is reserved for scenario docs)
  - `examples/integration-testing/partner-retry.routes.yaml` (new)
  - `examples/integration-testing/README.md` (modified)
  - `docs/src/testing/index.md` (modified)
- **Steps:**
  1. README: validate `partner` target row with `count` (exact,
     non-negative), `method`/`path` filters, `deadline` poll semantics,
     and one sentence: partner expectations are exact-count, not
     subset like message expectations.
  2. Example: `examples/integration-testing/partner-retry-route.yaml`
     (new) plus route file
     `examples/integration-testing/partner-retry.routes.yaml` (new)
     mirroring the Task 2.3 flagship exactly: pinned listener,
     `error_handler.retry` onto `${env:PARTNER_URL}/order`, partner
     declared only via action refs with `bindVar: PARTNER_URL`, fault
     then healthy entries, send + client-role receive + partner
     receive + validate `{count: 2, path: /order}` with `deadline`.
  3. Examples README row and book testing-page paragraph: failure-path
     testing is now expressible (delay, fault, times) and assertable
     (polled exact counts).
  4. Verify the example green: `cargo run -q -p camel-cli --features
     integration-http -- test
     examples/integration-testing/partner-retry-route.yaml` exits
     0 with all scenario steps passed; verify a scratch copy with
     `count: 99` exits 1 with `validation-mismatch`; delete the
     scratch.
- **Tests:**
  - name: example both directions; setup: example committed; action:
    run green variant and broken scratch; assert: exit 0 then exit 1
    with `validation-mismatch` in output; command: the two runs above;
    expected: as asserted.
- **Acceptance:**
  - Green run output shows the validate step passing.
  - `cargo test -p camel-cli --test lint_corpus` exits 0.
  - `cargo xtask lint-context-citations` exits 0.
- [x] 2.4

