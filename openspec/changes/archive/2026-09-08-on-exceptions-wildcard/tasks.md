# Tasks: on-exceptions-wildcard

## Task 1 — Accept `"*"` as a reserved kind token in compile_error_handler

- **ID**: oe-wc-1
- **Description**: Teach the DSL compiler to accept `kind: "*"` and build a
  matches-all clause matcher, keeping named-kind validation unchanged.

- **Files**:
  - `crates/camel-dsl/src/compile.rs` (modified)

- **Steps**:
  1. In `ensure_known_exception_kind` (crates/camel-dsl/src/compile.rs:881),
     return `Ok(())` early when `kind == "*"` before the
     `supported_exception_kinds().contains(&kind)` check. Leave the
     unknown-kind error message for all other names unchanged.
  2. In `exception_kind_matches` (crates/camel-dsl/src/compile.rs:912), add
     a first arm `"*" => true` before the named-variant arms.
  3. Verify no other gate rejects `"*"`: `compile_error_handler` clause
     validation (line ~758) only requires `kind.is_some() ||
     message_contains.is_some()`, which `"*"` satisfies; the matcher closure
     (line ~780) calls `exception_kind_matches(expected, e)` for `kind_ok`,
     so step 2 makes the clause match every variant. `message_ok` still
     ANDs with `kind_ok` when `message_contains` is present.
  4. Add unit tests in the `mod tests` of `compile.rs` (pattern: existing
     `exception_kind_matches_consumer_stopping` at line ~3112):
     - `wildcard_kind_passes_validation`: `ensure_known_exception_kind("*")`
       is `Ok(())`.
     - `wildcard_kind_matches_every_variant`: `exception_kind_matches("*", e)`
       is `true` for `ValidationError("x")`, `ProcessorError("x")`,
       `Io("x")`, and `CircuitOpen("x")`.
     - `unknown_kind_still_rejected`: `ensure_known_exception_kind("NoSuchKind")`
       is `Err` with a message containing `unknown exception kind`
       (regression guard — passes before and after this change).
     - `wildcard_narrowed_by_message_falls_through`: call
       `compile_error_handler` with a `DeclarativeErrorHandler` whose
       `on_exceptions` has TWO clauses in order — first
       `kind: Some("*".into())`, `message_contains: Some("timeout".into())`;
       second `kind: Some("*".into())`, `message_contains: None`. Assert
       the first policy's `matches` closure is `true` for
       `Io("connection timeout")` and `false` for `Io("disk full")`, and
       the second policy's `matches` closure is `true` for `Io("disk full")`
       (this pins spec scenario 4's dispatch: narrowed wildcard first,
       generic wildcard catches the fall-through).
     - `wildcard_policy_matcher_is_catch_all_and_conjuncts_message`: build an
       `ErrorHandlerConfig` by calling `compile_error_handler` directly with
       a `DeclarativeErrorHandler` (crates/camel-dsl/src/model.rs) whose
       `on_exceptions` is one clause with `kind: Some("*".into())` and
       `message_contains: Some("timeout".into())` — do NOT use the builder
       API here, the test must exercise the declarative compiler path;
       assert the policy `matches` closure returns `true` for
       `Io("connection timeout")` and `false` for `Io("disk full")`; then
       rebuild with `message_contains: None` and assert `true` for
       `ValidationError("anything")`.

- **Tests** (executable specs):
  - name: `wildcard_kind_passes_validation`
    - setup: `crates/camel-dsl/src/compile.rs` test module exists.
    - action: call `ensure_known_exception_kind("*")`.
    - assert: returns `Ok(())`.
    - command: `cargo test -p camel-dsl wildcard_kind_passes_validation`
    - expected: fails before step 1, passes after.
  - name: `wildcard_kind_matches_every_variant`
    - setup: `exception_kind_matches` in scope.
    - action: call it with `"*"` against `ValidationError`, `ProcessorError`,
      `Io`, `CircuitOpen` values.
    - assert: all four calls return `true`.
    - command: `cargo test -p camel-dsl wildcard_kind_matches_every_variant`
    - expected: fails before step 2, passes after.
  - name: `unknown_kind_still_rejected`
    - action: call `ensure_known_exception_kind("NoSuchKind")`.
    - assert: `Err` whose message contains `unknown exception kind`.
    - command: `cargo test -p camel-dsl unknown_kind_still_rejected`
    - expected: passes before AND after (regression guard).
  - name: `wildcard_narrowed_by_message_falls_through`
    - action: `compile_error_handler` with two wildcard clauses (first
      narrowed by `message_contains: "timeout"`, second generic).
    - assert: first policy `matches` is `true` for `Io("connection timeout")`
      and `false` for `Io("disk full")`; second policy `matches` is `true`
      for `Io("disk full")`.
    - command: `cargo test -p camel-dsl wildcard_narrowed_by_message_falls_through`
    - expected: fails before steps 1–2, passes after.
  - name: `wildcard_policy_matcher_is_catch_all_and_conjuncts_message`
    - action: compile a clause `kind: "*", message_contains: "timeout"` into
      a policy; invoke its `matches` closure.
    - assert: `true` for `Io("connection timeout")`, `false` for
      `Io("disk full")`; with `message_contains` removed, `true` for
      `ValidationError("anything")`.
    - command: `cargo test -p camel-dsl wildcard_policy_matcher_is_catch_all`
    - expected: fails before steps 1–2, passes after.

- **Acceptance**:
  - `cargo test -p camel-dsl --lib` exits 0.
  - `cargo clippy -p camel-dsl -- -D warnings` exits 0.
  - `cargo fmt --check` reports no changes needed for `compile.rs`.

- [x] oe-wc-1

## Task 2 — End-to-end integration: wildcard clause owns the HTTP response for every error kind

- **ID**: oe-wc-2
- **Description**: Prove at HTTP level that one wildcard clause with
  `handled: true` + `retry.handled_by` shapes every error kind's response,
  and that a specific clause placed first wins.

- **Files**:
  - `crates/camel-test/tests/on_exceptions_wildcard_test.rs` (new)

- **Steps**:
  1. Create the test file with the harness pattern from
     `crates/camel-test/tests/integration_test.rs` (CamelTestContext with
     `with_component(HttpComponent::new())`, `with_direct()`, AND
     `.with_mock()` — step 5 needs `mock:shaped`). Get distinct free ports
     from the repo helper `tests/support/mod.rs::stage_http_listener()`
     (call `install_crypto_provider()` first if the helper requires it) —
     do not hardcode ports. Use POST with `.body(...)` for body-carrying
     requests (repo convention; GET bodies are not reliable).
  2. Handler route `direct:shaper`: set body to `"shaped"`, set header
     `X-Custom` to the STRING `"yes"` (string value — non-string values are
     dropped by the reply finaliser, bd rc-lidtk), set header
     `CamelHttpResponseCode` to `422`.
  3. Main route `http://127.0.0.1:<port A>/probe` with a step that fails
     with `ValidationError("schema mismatch")` for requests whose body
     contains `invalid`, else fails with `ProcessorError("boom")` (single
     process_fn reading the request body).
     Error handler built with the camel-builder API mirroring the compiled
     clause shape: `ErrorHandlerConfig::log_only().on_exception(|_e| true).handled_by("direct:shaper").handled(true).build()`.
  4. Test `wildcard_owns_response_for_every_kind`: send both request
     bodies; assert both responses have status 422, header `x-custom ==
     "yes"`, and body `"shaped"`.
  5. Ordering test `specific_clause_precedes_wildcard`: main route
     `http://127.0.0.1:<port B>/probe` with first clause
     `kind: Io` semantics via `on_exception(|e| matches!(e, CamelError::Io(_))).continued(true)`
     and second clause the wildcard from step 3; handler route
     `direct:generic-shaper` (same shaping as `direct:shaper`); a trailing
     step after the failing one sets body to `"recovered"` (runs only on
     the continued path). Failing step raises `Io("disk")` for body
     containing `io` and `ValidationError("schema mismatch")` otherwise.
     Assert: for the `io` body the response body is `"recovered"` and
     status is 200; for the invalid body the response is the shaper's 422 /
     `"shaped"` / `x-custom: yes`. Use distinct mock endpoints or direct
     handler routes to also assert the generic shaper received no exchange
     on the `io` path (register the shaper as a `direct:` route writing to
     a `mock:` endpoint via `to("mock:shaped")` in the handler route, then
     `assert_exchange_count(0)` / `(1)`).
  6. JSON compile test `wildcard_compiles_from_json`: write a JSON route
     literal (mirroring the spec scenario) with `error_handler` →
     `on_exceptions: [{"kind": "*", "handled": true, "retry": {"handled_by":
     "direct:shaper", "max_attempts": 1}}]`, parse it with the public
     `camel_dsl::json::parse_json(&str)` — note `parse_json` PERFORMS the
     compilation and returns `Vec<RouteDefinition>` directly (json.rs:54-61);
     there is no separate compile call. Destructure the single
     `RouteDefinition` from the returned vec and read its policies via the
     public accessor `RouteDefinition::error_handler_config()`
     (camel-core route_definition.rs:555 — the raw field is `pub(crate)`).
     Assert exactly one policy, `matches` closure `true` for both
     `ValidationError` and `ProcessorError`, and `handled_by == Some(
     "direct:shaper")`.

- **Tests** (executable specs):
  - name: `wildcard_owns_response_for_every_kind`
    - setup: running HTTP consumer route with wildcard clause + shaper route.
    - action: POST with body `invalid`, POST with body `other`.
    - assert: both responses: status 422, header `x-custom == "yes"`, body
      `"shaped"`.
    - command: `cargo test -p camel-test --test on_exceptions_wildcard_test wildcard_owns`
    - expected: passes before AND after Task 1 — these HTTP tests use the
      builder API catch-all, which the engine already supports; they pin
      engine behavior, not the new declarative path.
  - name: `specific_clause_precedes_wildcard`
    - action: POST with body `io`; POST with body `invalid`.
    - assert: `io` → body `"recovered"`, status 200, `mock:shaped` count 0;
      `invalid` → 422, body `"shaped"`, `mock:shaped` count 1.
    - command: `cargo test -p camel-test --test on_exceptions_wildcard_test specific_clause`
    - expected: passes before AND after Task 1 (ordering is engine behavior
      being pinned via the builder API, not new code).
  - name: `wildcard_compiles_from_json`
    - action: parse the JSON literal of step 6 with `camel_dsl::json::parse_json`
      (which compiles and returns `Vec<RouteDefinition>`); destructure the
      single definition; read policies via `error_handler_config()`.
    - assert: one policy; `matches(ValidationError(..))` and
      `matches(ProcessorError(..))` both `true`; `handled_by` preserved.
    - command: `cargo test -p camel-test --test on_exceptions_wildcard_test wildcard_compiles_from_json`
    - expected: fails before Task 1, passes after.

- **Acceptance**:
  - `cargo test -p camel-test --test on_exceptions_wildcard_test` exits 0.
  - `cargo clippy -p camel-test --all-targets -- -D warnings` exits 0.
  - The three test names above appear and pass.
  - Spec scenario 4 (narrowed wildcard → generic wildcard) is executed
    directly by Task 1's `wildcard_narrowed_by_message_falls_through`; this
    task adds its observable HTTP-side analogue via the two-clause
    `specific_clause_precedes_wildcard` fallback shape.

- [x] oe-wc-2

## Task 3 — Document the wildcard clause

- **ID**: oe-wc-3
- **Description**: Cover `kind: "*"` in the user-facing error-handling doc:
  placement, precedence, and the full-response-ownership pattern.

- **Files**:
  - `docs/src/concepts/error-handling.md` (modified)

- **Steps**:
  1. Locate the `on_exceptions` reference near line 202 (the `kind:
     "ProcessorError"` example) and the section describing clause matching.
  2. Add a subsection `### Catch-all clause (kind: "*")` stating: `"*"`
     matches every error kind; declare it LAST because evaluation is
     first-match-wins; combining with `message_contains` narrows by
     message; the pattern `kind: "*"` + `handled: true` +
     `retry.handled_by` gives the handler route full ownership of the HTTP
     response (status, body, headers), with a YAML example mirroring the
     spec scenario (string-valued custom headers, `CamelHttpResponseCode`).
  3. Cross-reference the do_try catch wildcard (`exception: ["*"]`) as the
     segment-scoped equivalent.

- **Tests** (executable specs):
  - name: `wildcard-doc-section`
    - setup: docs build (`docs/src/concepts/error-handling.md` in the mdbook
      tree).
    - action: render check — the section heading exists and the example
      block is valid YAML.
    - assert: `grep -c 'Catch-all clause' docs/src/concepts/error-handling.md`
      returns ≥ 1; the YAML example contains `kind: "*"`, `handled: true`,
      and `handled_by`.
    - command: `grep -n 'Catch-all clause' docs/src/concepts/error-handling.md`
    - expected: no hits before, ≥1 hit after.

- **Acceptance**:
  - The section exists with the three elements above.
  - `mdbook build docs` exits 0 (run from the worktree; skip only if mdbook
    is not installed — then state that explicitly in the task report).
  - `grep -c 'kind: "\*"' docs/src/concepts/error-handling.md` returns ≥ 1.
  - `grep -c 'handled: true' docs/src/concepts/error-handling.md` returns ≥ 1.
  - `grep -c 'handled_by' docs/src/concepts/error-handling.md` returns ≥ 1.

- [x] oe-wc-3
