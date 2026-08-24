# Tasks: bean-test-registry

Single-phase change (no `## Phase N` headings by design; see design.md).

## camel-cli (+1 camel-core accessor)

### Task 1: Mechanical prep — extract document tests, add camel-core accessor

**Files:**
- `crates/camel-cli/src/commands/test/document.rs` (modified)
- `crates/camel-cli/src/commands/test/document_tests.rs` (new — extracted test module, bd rc-jrxr)
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified)

**Steps:**
1. Extract the inline `#[cfg(test)] mod tests` of `crates/camel-cli/src/commands/test/document.rs` (currently ~683 lines, file total 1071) verbatim to `crates/camel-cli/src/commands/test/document_tests.rs` and replace it with `#[cfg(test)] #[path = "document_tests.rs"] mod tests;` (bd rc-jrxr disposition; pure move, no test edits; move any test-only helper imports along so the module compiles standalone).
2. In `crates/camel-core/src/lifecycle/application/route_definition.rs`, add a public accessor mirroring the existing `steps()` accessor (route_definition.rs:403): `pub fn circuit_breaker_fallback(&self) -> &[BuilderStep]` returning `&self.circuit_breaker_fallback`. No other camel-core change.
3. Add one inline unit test next to the existing route_definition.rs tests: `circuit_breaker_fallback_accessor_returns_steps` — build `RouteDefinition::new("direct:start", vec![]).with_circuit_breaker_fallback(vec![BuilderStep::To("mock:out".into())])` → accessor returns slice of length 1.

**Tests:** (mechanical; all pass immediately)
- `circuit_breaker_fallback_accessor_returns_steps`: as step 3. `cargo test -p camel-core --lib circuit_breaker_fallback` passes.
- Existing document tests unchanged: `cargo test -p camel-cli --lib` passes with the same test count as before extraction.

**Acceptance:**
- `cargo test -p camel-cli --lib` passes (count unchanged from pre-extraction).
- `cargo test -p camel-core --lib circuit_breaker_fallback` passes.
- `cargo fmt --check --all`, `cargo clippy -p camel-cli -- -D warnings`, `cargo clippy -p camel-core -- -D warnings` exit 0.
- `document.rs` is < 400 lines after extraction.

- [x] 1

### Task 2: Parsing layer — `beans:` block in test documents

**Files:**
- `crates/camel-cli/src/commands/test/document.rs` (modified)
- `crates/camel-cli/src/commands/test/document_tests.rs` (modified — add tests)

**Steps:**
1. In `document.rs`, add `#[derive(Deserialize, Debug, Clone, PartialEq)] #[serde(deny_unknown_fields, rename_all = "camelCase")] pub struct BeanDeclDoc { pub kind: BeanKindDoc, pub methods: Option<Vec<String>>, pub config: Option<BTreeMap<String, String>> }` and `#[derive(Deserialize, Debug, Clone, Copy, PartialEq)] #[serde(rename_all = "camelCase")] pub enum BeanKindDoc { Echo, SetBody, Fail }`.
2. Add field `pub beans: Option<BTreeMap<String, BeanDeclDoc>>` to `TestDocument` (serde `deny_unknown_fields, rename_all = "camelCase"`, matching existing fields).
3. Extend the post-parse validation in `parse_test_document` with a beans phase (mirror the intercepts validation structure at document.rs:334-356): for each `(name, decl)` in `beans` (BTreeMap iteration order): (a) `name.trim().is_empty()` → error `bean names must be non-blank`; (b) `decl.methods == Some(vec![])` → error `bean {name}: methods must be non-empty or omitted`; (c) any `methods` entry with `entry.trim().is_empty()` → error `bean {name}: method names must be non-blank`; (d) per-kind config validation: `Echo` → config must be `None` or empty, any key → error `bean {name}: config key {key} is not valid for kind echo`; `SetBody` → config must contain a `body` key (missing → error `bean {name}: kind setBody requires config key body`; any extra key → same not-valid error as Echo); `Fail` → only `message` key allowed (extra → not-valid error naming kind fail).
4. Add one `TestDocError` variant `InvalidBeans(String)` (message-carrying, Display prints the message verbatim); all beans validation errors construct it with the precise messages above.
5. Add accessor `impl TestDocument { pub fn bean_decls(&self) -> Option<&BTreeMap<String, BeanDeclDoc>> { self.beans.as_ref() } }` (validation ran eagerly in `parse_test_document`; accessor is infallible).

**Tests:** (in `document_tests.rs`; all fail before steps 1-5, pass after)
- `beans_absent_keeps_behavior`: parse a minimal doc without `beans` → `bean_decls()` is `None`, no error.
- `beans_setbody_parses`: parse doc with `beans: {enricher: {kind: setBody, config: {body: stubbed}}}` → `bean_decls()` returns decl with `kind == BeanKindDoc::SetBody`, `methods == None`, `config` contains `body == "stubbed"`.
- `beans_unknown_kind_rejected`: `beans: {x: {kind: teleport}}` → `parse_test_document` errors; assert message contains `teleport` AND mentions all three supported kinds (`echo`, `setBody`, `fail`) in one message (serde's unknown-variant error already lists renamed variants — preserve/classify it; if the serde message alone is insufficient, map to `InvalidBeans` with a message that lists them).
- `beans_blank_name_rejected`: `beans: {"  ": {kind: echo}}` → error, message contains `bean names must be non-blank`.
- `beans_empty_methods_rejected`: `beans: {x: {kind: echo, methods: []}}` → error, message contains `methods must be non-empty or omitted`.
- `beans_blank_method_entry_rejected`: `beans: {x: {kind: echo, methods: ["a", ""]}}` → error, message contains `method names must be non-blank`.
- `beans_setbody_missing_body_rejected`: `beans: {x: {kind: setBody, config: {}}}` → error, message contains `requires config key body`.
- `beans_config_variant_pins` (requirement-text pin, one test, three arms): `echo` with `config: {body: y}` → error contains `not valid for kind echo`; `setBody` with `{body: y, extra: z}` → error contains `not valid for kind setBody`; `fail` with `{message: m, other: o}` → error contains `not valid for kind fail`.
- `beans_nested_unknown_field_rejected`: `beans: {x: {kind: echo, metod: [a]}}` → document error mentioning the unknown field (`metod`).

**Acceptance:**
- `cargo test -p camel-cli --lib` passes (count grows by 9: 1 kept-behavior + 8 above).
- `cargo fmt --check --all` and `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 2

### Task 3: Stub beans + runner wiring (registry before boot)

**Files:**
- `crates/camel-cli/src/commands/test/beans.rs` (new, incl. inline `#[cfg(test)]` unit tests)
- `crates/camel-cli/src/commands/test/mod.rs` (modified — add `mod beans;`)
- `crates/camel-cli/src/commands/test/runner.rs` (modified)
- `crates/camel-cli/tests/test_beans.rs` (new)

**Steps:**
1. Create `beans.rs` with `pub(crate) enum StubKind { Echo, SetBody { body: String }, Fail { message: String } }` and `pub(crate) struct StubBean { name: String, methods: Vec<String>, kind: StubKind }`. Implement `camel_bean::BeanProcessor` for `StubBean`: `call` — `Echo` returns `Ok(())`; `SetBody` sets `exchange.input.body` to the configured `String`; `Fail` returns `Err(CamelError::ProcessorError(message))` where message = configured value or exactly `fail bean {name}`. `methods()` returns `self.methods.clone()`. `method_params()` default (None). `on_start`/`on_stop` default (Ok). Add `pub(crate) fn stub_from_decl(name: &str, decl: &BeanDeclDoc, invoked: &[String]) -> StubBean` building the `methods` list: if `decl.methods` is `Some(list)` use the declared list; else use `invoked` (methods the routes call on this bean, deduplicated, order of first appearance).
2. Create `pub(crate) fn collect_bean_calls(defs: &[RouteDefinition]) -> Vec<(String, String)>` in `beans.rs`: walk every `BuilderStep` in each definition's `steps()` AND `circuit_breaker_fallback()`; the match MUST be exhaustive with NO `_` catch-all arm — every `BuilderStep` variant that holds nested `Vec<BuilderStep>` (at minimum `BuilderStep::Cache { on_miss, .. }`, route_definition.rs:281-288; enumerate the full enum and cover every nested location) is recursed into, `BuilderStep::Bean { name, method }` pushes `(name, method)`, all other variants contribute nothing. An exhaustive match makes future nested-step variants a compile error here instead of a silent gap.
3. In `runner.rs`, change the boot order for `run_test_doc`: hoist the existing `load_routes(doc, doc_dir)` call (runner.rs:102-141, ctx-free) to BEFORE `boot_context` so definitions are parsed once; the existing `add_route_definition` call reuses the same parsed definitions. Then: if `doc.bean_decls()` is `Some(decls)`, call `collect_bean_calls(&defs)`, cross-validate (if `decl.methods` is `Some(list)`: every `(name, method)` in the collected calls with that bean name must have `method` in `list`, else `TestDocError::InvalidBeans("bean {name}: method {method} is not declared")` → doc_error exit 2 before boot), build `camel_bean::BeanRegistry::new()`, register each `stub_from_decl(name, decl, invoked_for_that_bean)`, and pass `Some(Arc::new(std::sync::Mutex::new(registry)))` to `boot_context`, which threads `builder.beans(...)` (context_builder.rs:125). Absent `beans` → current behavior unchanged (no registry wiring).
4. `boot_context` signature gains the registry parameter (single call site at `run_test_doc`).

**Tests:**
- beans.rs inline unit tests (design's "stub impls + unit tests"): `stub_from_decl_explicit_methods_uses_declaration` (decl with `methods: Some([a,b])` → `methods()` == `[a,b]` regardless of `invoked`); `stub_from_decl_wildcard_dedupes_invoked` (decl without methods, `invoked: [m1, m2, m1]` → `methods()` == `[m1, m2]`); `collect_bean_calls_walks_fallback_and_cache_on_miss` (build `RouteDefinition` with steps `[Bean{x,m}, Cache{ on_miss: vec![Bean{y,n}], .. }]` (construct per actual variant shape) and `with_circuit_breaker_fallback(vec![Bean{z,o}])` → collected pairs include `(x,m)`, `(y,n)`, `(z,o)`); `fail_default_message_exact` (`StubKind::Fail` via `stub_from_decl("gate", fail-decl, [])`, call("check", &mut exchange) → Err message == `fail bean gate`).
- Integration tests in `crates/camel-cli/tests/test_beans.rs` (write FIRST, TDD; follow test_intercepts.rs conventions: temp_dir, run helper, `#[tokio::test(flavor = "multi_thread")]`, `// allow-unwrap`, DSL `steps: - to:` form; bean steps use `bean: {name: x, method: y}` DSL form):
- `bean_route_without_registry_fails_today` (RED guard, write FIRST, run, observe the message): inline route with `bean: {name: enricher, method: enrich}` step and NO `beans:` block → run_test_doc returns doc_error containing `Bean not found: enricher` (documents pre-change behavior; stays passing after wiring).
- `setbody_stub_transforms_body`: doc with `beans: {enricher: {kind: setBody, config: {body: stubbed}}}`, route `from: direct:start` steps `bean: {name: enricher, method: enrich}` then `to: mock:out`, input `{to: direct:start, body: "x"}`, `expects: {mock:out: {count: 1, bodies: ["stubbed"]}}` → run passes. RED before wiring: fails `Bean not found: enricher`.
- `echo_stub_passes_through`: doc with `beans: {gate: {kind: echo}}`, route with `bean: {name: gate, method: whatever}` then `to: mock:out`, input body `x`, `expects: {mock:out: {count: 1, bodies: ["x"]}}` → passes.
- `fail_stub_surfaces_doc_error`: doc with `beans: {gate: {kind: fail, config: {message: boom}}}`, route `bean: {name: gate, method: check}` then `to: mock:out`, one input → run_test_doc returns doc_error whose message contains `boom`; result carries no endpoint evaluations.
- `fail_stub_default_message`: same but `beans: {gate: {kind: fail}}` → doc_error message contains `fail bean gate`.
- `undeclared_method_rejected_before_boot`: doc with `beans: {enricher: {kind: echo, methods: [enrich]}}`, route invoking `bean: {name: enricher, method: transform}` → doc_error containing `method transform is not declared`, message does NOT contain `Bean not found` (proves cross-validation fired before boot).

**Acceptance:**
- `cargo test -p camel-cli --lib beans` passes (4 unit tests).
- `cargo test -p camel-cli --test test_beans` passes 6/6.
- `cargo test -p camel-cli --test test_intercepts --test test_runner` still pass (no regression from boot-order refactor).
- `cargo fmt --check --all`, `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 3

### Task 4: Execution matrix — validation, non-interference, composition

**Files:**
- `crates/camel-cli/tests/test_beans.rs` (modified — add tests)

**Steps:**
1. Add execution-level tests covering the remaining spec scenarios (reuse Task 3 helpers and `tests/common/mod.rs` `spawn_camel_run`/`wait_exit_bounded` where subprocess assertion is needed).
2. Add a comment block at the top of test_beans.rs mapping every delta scenario to its owning test (spec scenario name → test fn), including the requirement-pins from Task 2 (config variants) and both MODIFIED scenarios.

**Tests:**
- `wildcard_accepts_route_methods`: doc `beans: {gate: {kind: echo}}` (no methods) + route invoking TWO bean steps on `gate` with different methods `m1`, `m2` then `to: mock:out` → passes, mock count 1.
- `multiple_beans_one_document`: doc `beans: {a: {kind: setBody, config: {body: first}}, b: {kind: echo}}`, route invoking `bean: {name: a, method: m1}` then `bean: {name: b, method: m2}` then `to: mock:out` → passes, mock records body `first` count 1.
- `blank_bean_name_exit_2`: CLI subprocess (`camel test`) on a temp doc with `beans: {"  ": {kind: echo}}` → exit code 2, stderr contains `non-blank` (pins exit-code mapping through test.rs:182-207).
- `input_delivery_failure_skips_evaluation`: CLI subprocess on the fail-stub doc from Task 3 → exit 2, output contains `boom`, and stdout has NO line for `mock:out` starting with `PASS` or `FAIL` (pins MODIFIED exit-codes scenario).
- `camel_run_ignores_beans_block`: subprocess — write temp `probe.test.yaml` containing `beans: {x: {kind: teleport}}` (intentionally invalid) next to a valid `routes.yaml` (`from: direct:start` → `to: log:out`); run the `camel_run` binary against `routes.yaml` (same invocation shape as `camel_run_ignores_intercepts_block` in test_intercepts.rs) → route runs successfully; assert stdout AND stderr each contain none of: `probe.test.yaml`, the invalid kind name `teleport`, the fragment `unknown kind`.
- `intercepts_and_beans_compose`: doc with `intercepts: {kafka:orders: {skipTo: mock:orders}}`, `beans: {gate: {kind: setBody, config: {body: stamped}}}`, route A `from: direct:a` steps `to: kafka:orders`, route B `from: direct:b` steps `bean: {name: gate, method: mark}` then `to: mock:out`, inputs to `direct:a` (body `k`) and `direct:b` (body `z`), `expects: {mock:orders: {count: 1, bodies: ["k"]}, mock:out: {count: 1, bodies: ["stamped"]}}` → passes.
- `multi_doc_with_beans_isolated`: two docs in one CLI invocation — `a.test.yaml` with `beans: {a1: {kind: setBody, config: {body: aa}}}` + bean route → `mock:oa`, `b.test.yaml` WITHOUT beans + plain route → `mock:ob`; both pass (registry is per-doc; no leak).

**Acceptance:**
- `cargo test -p camel-cli --test test_beans` passes 6 + 7 = 13/13.
- `cargo test -p camel-cli` (all integration tests) passes.
- `cargo fmt --check --all`, `cargo clippy -p camel-cli -- -D warnings` exit 0.
- Scenario-mapping comment block present and complete (every delta scenario has an owner).

- [x] 4

### Task 5: Docs, example, and citations

**Files:**
- `docs/src/testing/index.md` (modified)
- `examples/yaml-dsl/config/beans-demo.yaml` (new)
- `examples/yaml-dsl/config/beans-demo.test.yaml` (new)
- `crates/camel-cli/CONTEXT.md` (modified)

**Steps:**
1. In `docs/src/testing/index.md`, extend the `## Declarative camel test` section with `### Bean stubs` (sibling of `### Intercepts`): document the `beans:` block (kinds table `echo`/`setBody`/`fail` with config keys and defaults, `methods` wildcard vs explicit semantics incl. cross-validation exit 2, fail's exit-2 semantics and exact default message `fail bean <name>`), linking the naming/behavior to the `bean:` step. Follow STE writing rules (docs/src/testing is durable prose).
2. Create `examples/yaml-dsl/config/beans-demo.yaml`: production-shaped route `from: direct:orders` with steps `bean: {name: validator, method: validate}` then `bean: {name: enricher, method: enrich}` then `to: log:processed` — routes ONLY (no test doubles in the production file).
3. Create `examples/yaml-dsl/config/beans-demo.test.yaml`: `routeFiles: [beans-demo.yaml]` (doc-relative resolution — the doc sits in the same directory; mirrors intercepts-demo.test.yaml), `intercepts: {log:processed: {skipTo: mock:processed}}`, `beans: {validator: {kind: echo}, enricher: {kind: setBody, config: {body: enriched}}}`, one input body `order-1`, `expects: {mock:processed: {count: 1, bodies: ["enriched"]}}`.
4. Verify end-to-end from the worktree: `cargo run -p camel-cli -- test examples/yaml-dsl/config/beans-demo.test.yaml` exits 0 with a PASS line for `mock:processed`.
5. In `crates/camel-cli/CONTEXT.md`, extend the test-command entry (where the intercepts pointer lives) with one sentence pointing to `docs/src/testing/index.md ### Bean stubs` for bean-stub declarative testing.
6. Confirm `docs/src/SUMMARY.md` already registers testing/index.md (Stage A did); no SUMMARY change needed unless structure demands it.

**Tests:** (executable verification, not #[test])
- `example-runs-green`: `cargo run -p camel-cli -- test examples/yaml-dsl/config/beans-demo.test.yaml` → exit 0, stdout contains `PASS` and `mock:processed`.
- `lint-context-citations`: `cargo xtask lint-context-citations` exits 0.

**Acceptance:**
- Both verification commands pass from the worktree.
- `cargo fmt --check --all` exits 0.
- Docs use STE (no AI-slop phrasing); example pair mirrors intercepts-demo conventions.

- [x] 5
