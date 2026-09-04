# Tasks: itest-send-method

## camel-integration-test

### Task 1.1: grammar field, token validation, and method resolution

**Files:**
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/doc_parse_test.rs` (modified, if that is where parse tests live; otherwise the file holding document parse unit tests)

**Steps:**
1. Add `method: Option<String>` to `RawSend` (document.rs:247 area). The struct already carries `deny_unknown_fields` and `rename_all = "camelCase"`; the field inherits both.
2. Add `method: String` to the `ScenarioAction::Send` variant (document.rs:92 area) with a doc comment: resolved method, explicit or inferred (`POST` with a body, `GET` without), uppercase.
3. Add a private pure predicate `fn is_http_token(s: &str) -> bool`: true when `s` is non-empty and every character is an ASCII alphanumeric or one of ``! #$%&'*+-.^_`|~``. No other dependency.
4. In the `"send"` match arm (document.rs:624 area), resolve the method after the serde parse: when `raw.method` is `Some`, trim it, apply `to_ascii_uppercase`, and require `is_http_token`; an invalid token returns the same `action_error` used for the missing deadline, with a message naming the requirement, for example ``send action `method` must be an HTTP token, got `P UT` ``. When `raw.method` is `None`, infer `POST` when `raw.body` is `Some` and `GET` otherwise. Pass the resolved string into `ScenarioAction::Send`.
5. Update the runner's `ScenarioAction::Send` destructure (runner.rs:237-271 area) to bind the new field with an underscore prefix for now. This keeps the crate compiling and clippy-clean; task 1.2 threads it. Extend the `endpoint_bindings(to)` call site (document.rs:133) only if the new field affects it. It does not: no change expected there.

**Tests:** (add to the document parse test module)
- `send_method_explicit_put_resolves`: setup: a send node with `method: PUT` and no body → action: load the document → assert: the action's resolved method equals `"PUT"`.
- `send_method_inferred_post_with_body`: setup: a send node with a body and no `method` → assert: resolved method equals `"POST"`.
- `send_method_inferred_get_bodyless`: setup: a send node with neither `method` nor `body` → assert: resolved method equals `"GET"`.
- `send_method_lowercase_normalizes`: setup: `method: delete` → assert: resolved method equals `"DELETE"`.
- `send_method_invalid_token_is_doc_validation`: setup: `method: "P UT"` → action: load → assert: the load error carries the action index, the same error variant the missing-deadline case produces, and classifies as doc validation. The CLI exit mapping is variant-generic (existing driver tests map this variant to exit 2), so variant equality is the feature-off acceptance proof. Command for all: `cargo test -p camel-integration-test send_method`.
- `send_method_token_predicate_accepts_and_rejects`: setup: the predicate → assert: accepts `PUT`, `X-Custom`, `PATCH2`; rejects `""`, `"P UT"`, `"PUT/"`, `"put;"`.

**Acceptance:**
- `cargo test -p camel-integration-test send_method` passes with the
  six tests (all carry the `send_method` prefix).
- `cargo test -p camel-integration-test` (no features) stays green: validation runs without the `http` feature.
- `cargo clippy -p camel-integration-test -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: message seam plumbing and adapter swap

**Files:**
- `crates/camel-integration-test/src/adapters.rs` (modified)
- `crates/camel-integration-test/src/runner.rs` (modified)
- `crates/camel-integration-test/src/adapters/http.rs` (modified)
- `crates/camel-integration-test/src/runner_test.rs` (modified: 2 construction sites)
- `crates/camel-integration-test/src/http_partner_test.rs` (modified: 5 construction sites)

**Steps:**
1. Add `pub method: String` to `OutgoingMessage` (adapters.rs:44-50) with a doc comment naming the resolved HTTP method for client-role sends.
2. Thread the field: the runner's send dispatch destructures `ScenarioAction::Send` (runner.rs:237-271 area) and builds `OutgoingMessage`; set `method` from the resolved action field (replace the task 1.1 underscore binding).
3. Update the remaining construction sites with the appropriate method string: `runner_test.rs` (2 sites) and `http_partner_test.rs` (5 sites). Stimulus sites that previously relied on the old inference use `GET` or `POST` to preserve their behavior.
4. Replace the inline inference at adapters/http.rs:466-469 (`let method = if msg.body.is_null() { Method::GET } else { Method::POST }`) with `Method::from_str(&msg.method)`. A grammar-validated token always parses. Keep the `expect`-free style of the file.

**Tests:**
- No new tests in this task. Regression: `cargo test -p camel-integration-test --features http` full suite stays green (existing tests prove behavior preserved through the threading).

**Acceptance:**
- `cargo build -p camel-integration-test --features http` exits 0.
- `cargo clippy -p camel-integration-test --all-targets --features http -- -D warnings` exits 0.
- The full suite stays green.

- [x] 1.2

### Task 1.3: end-to-end proof through the partner matcher

**Files:**
- `crates/camel-integration-test/tests/http_outbound_test.rs` (modified)

**Steps:**
1. Read `tests/http_outbound_test.rs` and follow its scenario-to-partner pattern (scenario `send` targets a partner endpoint, the partner holds a `ScriptedResponse` matcher, scenario `receive` consumes the parked receiver). Note: an unmatched request serves status 500 immediately (http.rs UNMATCHED_STATUS), it does not time out. The tests must therefore validate the scripted payload, not merely complete a receive.
2. Add two e2e tests where the partner matcher plus payload validation is the oracle:
   - `explicit_put_reaches_partner`: partner scripted response with `method: Some("PUT")`, status 200, body `put-ok`; scenario sends `method: PUT` with no body to that endpoint, receives, and validates the body equals `put-ok`.
   - `bodyless_post_reaches_partner`: partner scripted response with `method: Some("POST")`, status 200, body `post-ok`; scenario sends `method: POST` with no body, receives, and validates the body equals `post-ok`.
   Under the legacy `body?POST:GET` rule both sends would be `GET`, the scripted responses would not match, the partner would serve the 500 unmatched status with an empty body, and the body validations would fail: the tests prove the field end to end.

**Tests:** (the two e2e tests above are the task's tests)
- Command: `cargo test -p camel-integration-test --features http --test http_outbound_test`. Passes after task 1.3. Fails before the whole change.

**Acceptance:**
- The two e2e tests pass with `--features http`.
- `cargo test -p camel-integration-test --features http` full suite stays green.
- `cargo test -p camel-cli --test lint_corpus` stays green: no scenario document changed, so no new corpus diagnostics.

- [x] 1.3

### Task 1.4: README documentation for the send method field

**Files:**
- `crates/camel-integration-test/README.md` (modified)

**Steps:**
1. Locate the scenario vocabulary description and the `orders.test.yaml` example (README lines 7 and 25): the README, not the book, is the grammar reference for scenario actions.
2. Extend the `send` vocabulary sentence with the optional `method` field: uppercase normalization at load, token validation (doc-validation exit 2 on an invalid token), and the default inference (`POST` with a body, `GET` without).
3. Add `method: PUT` to the example's `send` block, keeping the example minimal.
4. Keep the existing style: short sentences.

**Tests:**
- `readme_documents_method`: Command: `grep -q 'method: PUT' crates/camel-integration-test/README.md` exits 0 (the example carries the field) and `grep -q 'uppercase' crates/camel-integration-test/README.md` exits 0 (the normalization is documented).

**Acceptance:**
- The README documents field, normalization, validation error, inference, and the example.
- No other files change.

- [x] 1.4

### Task 1.5: adoption surfaces (inbound method example)

**Files:**
- `examples/integration-testing/inbound-put.test.yaml` (new)
- `examples/integration-testing/routes/inbound-orders.yaml` (new)
- `examples/integration-testing/README.md` (modified)
- `docs/src/testing/index.md` (modified)

**Steps:**
1. Create `inbound-put.test.yaml` with `routes/inbound-orders.yaml` as
   its `routeFiles` entry. The route is the method oracle: it serves
   PUT /orders on the pinned loopback port 18097 (`httpMethod=PUT`
   from-URI discriminator) and answers `put-accepted`. The scenario
   sends `method: PUT` with no body to the SUT listener, receives the
   route's response (client role: the response parked by the send),
   validates the status is 200, and validates the body equals
   `put-accepted`. (The original direct-send-to-partner shape is not
   expressible: the client role dials the literal `:0` router key; bd
   rc-gz2r tracks that gap.)
2. Verify by running: `cargo run -p camel-cli --features integration-http -- test examples/integration-testing/inbound-put.test.yaml` exits 0 with PASS lines. A scratch copy with `method: GET` must fail the status validation (`expected 200, got 404`), proving the oracle discriminates.
3. Add a short section to `examples/integration-testing/README.md` describing the new document, the one command that runs it, and the pinned-port caveat in one sentence.
4. Add a `### Scenario documents` subsection at the end of `## Declarative camel test` in `docs/src/testing/index.md`: five to eight lines stating what scenario documents are (pointer to ADR-0069), the four actions, the optional `method` field with one example line, and that the crate README of `camel-integration-test` is the grammar reference.

**Tests:**
- `example_runs_green`: `cargo run -p camel-cli --features integration-http -- test examples/integration-testing/inbound-put.test.yaml` → exit 0, PASS lines.
- `oracle_discriminates`: a scratch copy with `method: GET` fails with `validation-mismatch: variable status: expected 200, got 404` (exit 1).
- `book_has_scenario_pointer`: `grep -q '### Scenario documents' docs/src/testing/index.md` → exit 0.

**Acceptance:**
- The example document runs green through the real CLI, and the GET
  scratch run is red.
- The example README documents it.
- The book carries the scenario pointer subsection.
- Corpus gate stays green (`cargo test -p camel-cli --test lint_corpus`).

- [x] 1.5
