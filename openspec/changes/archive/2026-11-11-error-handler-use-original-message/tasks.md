# Tasks: error-handler-use-original-message

<!-- Single-phase change: no `## Phase N` headings. design.md has no `## Phases` section. -->
<!-- Workers: do NOT `git commit` — the conductor makes ONE commit after review (mission order). -->
<!-- All cargo invocations run foreground inside the fleet containment wrapper (dispatch prompt carries the exact command form). -->

## camel-dsl

### Task 1.1: Write all new tests for the use_original_message wiring (red run)

**Files:**
- `crates/camel-dsl/src/compile.rs` (modified — test module only)
- `crates/camel-dsl/tests/declarative_compile_test.rs` (modified)
- `crates/camel-cli/tests/test_runner.rs` (modified)

**Steps:**
1. In the existing `mod tests` of `crates/camel-dsl/src/compile.rs`, add four
   unit tests modeled on `test_compile_error_handler_on_exceptions_order_preserved`
   (~line 2423). Each builds a full `DeclarativeErrorHandler` struct literal
   (all four fields: `dead_letter_channel`, `retry`, `on_exceptions`,
   `use_original_message` — no `..Default` shortcut) and calls
   `compile_error_handler(..).expect("compile should succeed")`:
   - `test_compile_error_handler_use_original_message_dlc_only`:
     `dead_letter_channel: Some("log:dlc".into())`, `retry: None`,
     `on_exceptions: None`, `use_original_message: true` → assert
     `config.use_original_message` is `true`.
   - `test_compile_error_handler_use_original_message_retry_only`:
     `retry: Some(DeclarativeRedeliveryPolicy { max_attempts: 2,
     initial_delay_ms: 10, multiplier: 2.0, max_delay_ms: 100,
     jitter_factor: 0.0 })`, `dead_letter_channel: None`,
     `on_exceptions: None`, `use_original_message: true` → assert
     `config.use_original_message` is `true` and `!config.policies.is_empty()`.
   - `test_compile_error_handler_use_original_message_on_exceptions`:
     `on_exceptions: Some(vec![DeclarativeOnException { kind: Some("Io".into()),
     message_contains: None, retry: None, steps: vec![], handled: Some(true),
     continued: None, handled_by: Some("log:io".into()) }])`,
     `dead_letter_channel: None`, `retry: None`, `use_original_message: true`
     → assert `config.use_original_message` is `true` and
     `!config.policies.is_empty()`.
   - `test_compile_error_handler_use_original_message_default_false`:
     `dead_letter_channel: Some("log:dlc".into())`, `retry: None`,
     `on_exceptions: None`, `use_original_message: false` → assert
     `!config.use_original_message`.
2. In `crates/camel-dsl/tests/declarative_compile_test.rs`, add a test
   `yaml_use_original_message_reaches_compiled_error_handler`:
   call `camel_dsl::parse_yaml_to_declarative` on a YAML string with one
   route (`id: yaml-uom`, `from: "direct:start"`, `steps: [{to: "mock:out"}]`,
   and `error_handler: {dead_letter_channel: "mock:dlc",
   use_original_message: true}`), take the first route, pass it to
   `camel_dsl::compile_declarative_route`, then assert
   `def.error_handler_config().expect("error handler config must compile")
   .use_original_message` is `true`.
3. In `crates/camel-cli/tests/test_runner.rs`, add two async tests using the
   existing `run(yaml, &dir)` helper (multi_thread tokio test, unique
   `temp_dir` tag), modeled on `route_files_resolve_relative_to_doc_dir`:
   - `use_original_message_routes_original_body_to_dlc`: document with the
     report's exact scenario — producer route `id: start` from
     `direct:start` with `steps: [{to: "seda:q"}]`; consumer route
     `id: consume` from `seda:q` with `error_handler: {dead_letter_channel:
     "mock:dlc", use_original_message: true}` and steps
     `[{set_body: "MUTATED"}, {validate: "${body} != 'MUTATED'"}]`;
     `inputs: [{to: "direct:start", body: "ORIGINAL-EVENT"}]`;
     `expects: {mock:dlc: {count: 1, bodies: ["ORIGINAL-EVENT"]}}` →
     assert `doc_error` is `None`, `endpoint_results` has length 1, and the
     single `er.outcome` is `Ok`.
   - `no_use_original_message_routes_mutated_body_to_dlc` (control): the
     same document WITHOUT `use_original_message`, expects
     `mock:dlc: {count: 1, bodies: ["MUTATED"]}` → same assertions.
4. RED RUN — run the three targeted commands and CAPTURE the failure output
   verbatim for the report (the three flag unit tests, the YAML boundary
   test, and `use_original_message_routes_original_body_to_dlc` must FAIL;
   `test_compile_error_handler_use_original_message_default_false` and the
   MUTATED control must PASS — they guard behavior that is already correct):
   - `cargo test -p camel-dsl --lib use_original_message`
   - `cargo test -p camel-dsl --test declarative_compile_test yaml_use_original_message`
   - `cargo test -p camel-cli --test test_runner original_body` and
     `cargo test -p camel-cli --test test_runner mutated_body`

**Tests:** (executable spec — the tests ARE the deliverable of this task)
- `test_compile_error_handler_use_original_message_dlc_only`: no setup →
  build struct literal, call `compile_error_handler` → assert
  `config.use_original_message == true`; command:
  `cargo test -p camel-dsl --lib use_original_message`; expected before
  fix: FAIL (`assertion failed: config.use_original_message`).
- `test_compile_error_handler_use_original_message_retry_only`: no setup →
  same → assert flag true and policies non-empty; same command; expected
  before fix: FAIL.
- `test_compile_error_handler_use_original_message_on_exceptions`: no
  setup → same → assert flag true and policies non-empty; same command;
  expected before fix: FAIL.
- `test_compile_error_handler_use_original_message_default_false`: no
  setup → same → assert flag false; same command; expected: PASS before
  and after (regression guard for the opt-out default).
- `yaml_use_original_message_reaches_compiled_error_handler`: no setup →
  `parse_yaml_to_declarative` → `compile_declarative_route` →
  `error_handler_config()` → assert flag true; command:
  `cargo test -p camel-dsl --test declarative_compile_test
  yaml_use_original_message`; expected before fix: FAIL.
- `use_original_message_routes_original_body_to_dlc`: no setup (inline
  document) → `run(yaml, &dir)` → assert doc_error None, 1 endpoint
  result, outcome Ok; command:
  `cargo test -p camel-cli --test test_runner original_body`; expected
  before fix: FAIL (DLC receives MUTATED, expected ORIGINAL-EVENT).
- `no_use_original_message_routes_mutated_body_to_dlc`: no setup → same →
  same assertions with bodies `["MUTATED"]`; command:
  `cargo test -p camel-cli --test test_runner mutated_body`; expected:
  PASS before and after (control — behavior already correct).

**Acceptance:**
- RED run output captured: the 5 fix-dependent tests fail, the 2 guards
  pass, with failure text quoted in the task report.
- `cargo fmt --check --all` exits 0 (test code formatted).
- No production (non-test) code changed in this task: `git diff --stat`
  shows only the three test files changed, and
  `git diff crates/camel-dsl/src/compile.rs` shows no hunks outside the
  test module.

- [x] 1.1

### Task 1.2: Apply the one-line compiler fix (green run)

**Files:**
- `crates/camel-dsl/src/compile.rs` (modified — one production line)

**Steps:**
1. At the END of `compile_error_handler` in
   `crates/camel-dsl/src/compile.rs` (immediately before `Ok(config)` at
   ~line 892, after the `retry` branch closing brace at ~line 890), insert
   exactly:
   ```rust
   // Copy the declarative flag once after all policy branches.
   config.use_original_message = def.use_original_message;
   ```
   Nothing else in the function changes; `def.use_original_message` is a
   `Copy` bool, so reading it after the earlier partial moves of
   `def.on_exceptions`/`def.retry` compiles (proven by this task's build).
2. GREEN RUN — re-run all five previously red tests and both guards, all
   green:
   - `cargo test -p camel-dsl --lib use_original_message`
   - `cargo test -p camel-dsl --test declarative_compile_test`
   - `cargo test -p camel-cli --test test_runner`
3. Confirm no regression in the surrounding suites:
   - `cargo test -p camel-dsl --lib`
   - `cargo test -p camel-dsl --test declarative_compile_test`
4. Lint the affected crates:
   - `cargo fmt --check --all`
   - `cargo clippy -p camel-dsl --all-targets -- -D warnings`
   - `cargo clippy -p camel-cli --all-targets -- -D warnings`

**Tests:** (executable spec — this task turns the red set green)
- All five tests from Task 1.1 that failed before the fix now PASS under
  the same commands (names and assertions unchanged — no test edits in
  this task).
- Both guards (`test_compile_error_handler_use_original_message_default_false`,
  `no_use_original_message_routes_mutated_body_to_dlc`) still PASS — the
  opt-out behavior is unchanged.

**Acceptance:**
- All three targeted test commands exit 0 with the five previously-red
  tests passing.
- `cargo fmt --check --all` exits 0.
- Both clippy commands exit 0.
- The production diff is exactly the one assignment line plus its comment
  (`git diff crates/camel-dsl/src/compile.rs` shows no other hunks
  outside the test module).
- Conductor sweep (conductor-run before the single commit, reported in the
  mission park per the order's verification list): `cargo test -p
  camel-dsl` (full crate, all targets) exits 0 and
  `bash scripts/gates-local.sh` exits 0 with
  passed/failed/skipped + `ran: X of Y` recorded.

- [x] 1.2
