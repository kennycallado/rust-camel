# Tasks: declarative-intercepts

Single-phase: camel-cli declarative surface consuming Stage A. No camel-core code changes.

## Task 1: intercepts parsing and eager InterceptRules construction

Files:
- crates/camel-cli/src/commands/test/document.rs (modified)

Steps:
1. Add `pub struct InterceptActionDoc` with `#[derive(Debug, Clone)]`, `#[serde(deny_unknown_fields, rename_all = "camelCase")]`, fields `skip_to: Option<String>` and `divert_copy_to: Option<String>`.
2. Add to `TestDocument`: `intercepts: Option<BTreeMap<String, InterceptActionDoc>>` (camelCase `intercepts`) and a private stored result field `intercept_rules_parsed: Option<InterceptRules>` (mirroring `settle_parsed`), with accessor `pub fn intercept_rules(&self) -> Option<InterceptRules>` returning a clone (`InterceptRules: Clone`). Import the Stage A types from `camel_core::intercept::{InterceptAction, InterceptRule, InterceptRules}` (public module; NOT a crate-root re-export).
3. Extend validation inside `parse_test_document`, after the existing expects/settle checks, iterating the map in `BTreeMap` order: (a) source key empty → error; (b) source key starts with `mock:` → error naming the key; (c) both action fields present → error naming the source key; (d) neither present → error naming the source key; (e) build `Vec<InterceptRule>` preserving map order, `SkipTo`/`DivertCopyTo` from the present field; (f) reject a target of exactly `"mock:"` or of the form `"mock:"` followed only by a query string (e.g. `mock:?x=1` — empty endpoint path before the `?`) with an error naming the source key; (g) call `InterceptRules::new(rules)` and surface its `CamelError::Config` message verbatim in a `TestDocError::InterceptInvalid(String)` when it rejects (non-`mock:` targets).
4. Add error variants to `TestDocError` with Display text (exit-2 contract): `InterceptEmptySource`, `InterceptMockSource { key: String }`, `InterceptActionKeys { key: String, problem: &'static str }` (problem = "both" | "neither"), `InterceptEmptyTargetPath { key: String }`, `InterceptInvalid(String)`.
5. Keep every existing parse behavior unchanged (route-source exclusivity, expects mandatory, body scalars, settle range, unknown top-level fields).

Tests (unit, in `document.rs` test module; command `cargo test -p camel-cli --lib` — if the lib has no test cfg for this module, place them in the existing module `#[cfg(test)]` block):
- `intercepts_skip_to_parses` — setup: doc YAML `intercepts: {kafka:orders: {skipTo: mock:orders}}` + minimal valid doc; action: `parse_test_document`; assert: Ok, `intercept_rules()` is Some, rule maps `kafka:orders` → `SkipTo { uri: "mock:orders" }`.
- `intercepts_divert_copy_to_parses` — same shape with `divertCopyTo: mock:audit`; assert DivertCopyTo variant.
- `intercept_action_both_keys_rejected` — action object with both keys; assert `Err` naming the source key, exit-2-classified error.
- `intercept_action_neither_key_rejected` — action object `{}`; assert Err naming the source key.
- `intercept_target_non_mock_rejected` — `skipTo: direct:orders`; assert Err whose text contains the Stage A message fragment `must start with 'mock:'` and names `direct:orders`.
- `intercept_source_mock_scheme_rejected` — key `mock:a`; assert Err naming `mock:a`.
- `intercept_source_empty_rejected` — key `""`; assert Err.
- `intercept_target_empty_path_rejected` — table-driven over both forms: `skipTo: "mock:"` and `skipTo: "mock:?x=1"` (empty endpoint path before the `?`); assert Err naming the source key for each.
- `intercept_action_unknown_field_rejected` — action with `replaceWith: mock:x`; assert `TestDocError::UnknownField`-classified Err.
- `intercepts_absent_keeps_behavior` — doc without `intercepts`; assert `intercept_rules()` is None and parse Ok.
- Existing document tests unchanged and passing.

Acceptance:
- `cargo test -p camel-cli --lib` passes including the new tests above.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] task-1-intercepts-parsing

## Task 2: runner applies stored rules via builder surface

Files:
- crates/camel-cli/src/commands/test/runner.rs (modified)
- crates/camel-cli/tests/test_intercepts.rs (new)

Steps:
1. Change `boot_context()` signature to `async fn boot_context(intercepts: Option<InterceptRules>) -> Result<(CamelContext, MockComponent), String>`; when Some, call `CamelContext::builder().with_intercept_rules(rules)` before `.build()`. Import `InterceptRules` from `camel_core::intercept`.
2. At the `boot_context()` call site in `run_test_doc`, pass `doc.intercept_rules()`.
3. No other runner changes: expects/settle/evaluation/exit codes untouched.

Tests (integration, `crates/camel-cli/tests/test_intercepts.rs`, tokio rt like `test_runner.rs`; command `cargo test -p camel-cli --test test_intercepts`):
- `skip_to_unregistered_component_passes` — setup: temp project, route `from: direct:start` → `to: kafka:orders` (YAML `routes:`), doc with `intercepts: {kafka:orders: {skipTo: mock:orders}}`, input `direct:start` body `"x"`, `expects: {mock:orders: {count: 1, bodies: ["x"]}}`; action: run `run_test_doc`; assert: no doc_error, all endpoint results Ok, exit path 0 semantics (outcome Ok(())). No kafka component registered anywhere.
- `intercept_target_and_expects_meet_on_endpoint` — setup: same as above with `expects: {mock:orders: {count: 1}}`; action: run; assert: `mock` handle `get_endpoint("orders")` recorded exactly 1 exchange (both surfaces resolved to endpoint name `orders`).
- Both tests MUST fail before Task 2 step 1–2 land (rules not applied → route load fails on unregistered kafka). Record that TDD red state by running them after writing only the test file.

Acceptance:
- `cargo test -p camel-cli --test test_intercepts` passes.
- `cargo test -p camel-cli --test test_runner` still passes (no regression).
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] task-2-runner-applies-rules

## Task 3: execution semantics matrix

Files:
- crates/camel-cli/tests/test_intercepts.rs (modified)

Steps:
1. Add the execution-semantics integration tests below to the file started in Task 2. No production code changes are expected; if a test exposes a genuine Stage A contract gap, STOP and report `stage-a-contract-gap: <details>` instead of patching camel-core.

Tests (command `cargo test -p camel-cli --test test_intercepts`):
- `divert_copies_while_real_seda_receives` — setup: route 1 `from: direct:start → to: seda:audit → to: mock:sink`, route 2 `from: seda:audit → to: mock:drained`; doc `intercepts: {seda:audit: {divertCopyTo: mock:audit}}`, `expects: {mock:audit: {count: 1}, mock:drained: {count: 1}, mock:sink: {count: 1}}`; action: run with one input; assert: all three endpoints satisfied (pipeline continues past the intercepted send), doc_error None.
- `divert_unregistered_fails_route_load` — setup: route `to: kafka:orders`, doc `intercepts: {kafka:orders: {divertCopyTo: mock:orders}}`; action: run; assert: doc_error Some containing `kafka` unresolvable (Stage A enriched ComponentNotFound) — document-error class, CLI exit 2 (run_tests maps any doc_error to exit 2; unchanged failure class).
- `source_query_params_are_significant` — setup: route `to: "kafka:orders?x=1"` (quote in YAML), doc intercepts key only `kafka:orders` (no query); action: run; assert: doc_error Some naming `kafka` (no rule match → resolution fails).
- `camel_run_ignores_intercepts_block` — setup: temp project with `Camel.toml` marker, production route `from: direct:start → to: log:done` in `config/routes.yaml`, sibling `orders.test.yaml` declaring an intentionally INVALID intercept block (`intercepts: {mock:result: {skipTo: mock:intercepted}}` — a `mock:` source that would fail validation loudly if ever parsed); action: launch the `camel run` subprocess path used by existing run tests (follow `run_watch_test_doc_test.rs` / `lint_test_doc_skip.rs` patterns for spawn/liveness/termination — subprocess output streams, no in-process MockComponent access); assert: the process starts and stays alive (liveness window), and neither stdout nor stderr names the test document, `intercepts`, or any intercept validation error; then terminate cleanly. Strong observability: had `camel run` parsed the test document, the invalid rule would surface its exit-2 validation error in output. The structural guard is discovery-side test-suffix skip (`is_test_document`).

Acceptance:
- `cargo test -p camel-cli --test test_intercepts` passes all 6 tests.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

- [x] task-3-execution-matrix

## Task 4: documentation surface

Files:
- docs/src/testing/index.md (modified)
- examples/yaml-dsl/config/intercepts-demo.test.yaml (new)
- examples/yaml-dsl/config/intercepts-demo.yaml (new — route file paired with the test doc)
- crates/camel-cli/CONTEXT.md (modified, only if a section on the test command exists; otherwise skip this file and note it)

Steps:
1. In `docs/src/testing/index.md`, append a new `## Declarative camel test` section (after the existing `## Route interception` section) containing an `### Intercepts` subsection: the YAML shape (`intercepts` map, exactly one of `skipTo`/`divertCopyTo`), skip-vs-divert semantics (skip = pre-resolution substitution, real component unneeded; divert = copy + real send, component must be in the lean set), the naming bridge (target `mock:orders` and expects key `mock:orders` resolve to endpoint `orders`), URI verbatim-matching note (query parameters significant), and failure classes (parse errors and route-load errors such as divert-unregistered are both document errors, exit 2 — unchanged). Cite ADR-0064 and the route-interception spec.
2. Create `examples/yaml-dsl/config/intercepts-demo.yaml` (route `from: direct:start → to: kafka:orders → to: mock:after` — kafka unregistered is the point) and `intercepts-demo.test.yaml` (`routeFiles: [intercepts-demo.yaml]`, one input, `intercepts: {kafka:orders: {skipTo: mock:orders}}`, `expects: {mock:orders: {count: 1}}`). Verify the example passes: `cargo run -p camel-cli -- test examples/yaml-dsl/config/intercepts-demo.test.yaml` exits 0 (run inside the worktree).
3. Update `crates/camel-cli/CONTEXT.md` only if it documents the test command surface; add one sentence pointing to the intercepts block with a spec citation. If no such section exists, skip and note it in the task report.

Tests:
- `example_intercepts_demo_passes` — name: manual verification step recorded in the task report; setup: the two new example files; action: `cargo run -p camel-cli -- test examples/yaml-dsl/config/intercepts-demo.test.yaml`; assert: exit 0, summary shows mock:orders passed. (No new Rust test file for this; the example IS the artifact.)
- `lint_context_citations` — action: `cargo xtask lint-context-citations`; assert: exit 0.

Acceptance:
- `cargo xtask lint-context-citations` exits 0.
- The example test doc passes via the CLI invocation above (exit 0).
- `cargo fmt --check` exits 0 (docs/examples only — trivially true unless Rust touched).

- [x] task-4-docs
