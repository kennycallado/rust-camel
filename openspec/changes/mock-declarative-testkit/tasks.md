# Tasks: mock-declarative-testkit

Single-phase change. Main crate: `crates/camel-cli` (package `camel-cli`).
Builds on change #1 API (merged): `expect_count`, `expect_minimum_count`,
`expect_body`, `expect_header`, `try_assert_satisfied`, `get_endpoint` on
`MockComponent`/`MockEndpointInner`.

YAML crate convention: the workspace uses `noyalib` (`compat-serde-yaml`
feature) — match the import style of `crates/camel-dsl/src/yaml.rs`; do NOT
add `serde_yaml`.

### Task 1: test document model + validation

**Files:**
- `crates/camel-cli/src/commands/test.rs` (new — module root: `pub mod document;`)
- `crates/camel-cli/src/commands/test/document.rs` (new)
- `crates/camel-cli/src/commands/mod.rs` (modified — add `pub mod test;`)
- `crates/camel-cli/Cargo.toml` (modified — add workspace deps `noyalib` (feature `compat-serde-yaml`) and `humantime` if absent)

**Steps:**
1. Create `commands/test.rs` containing only `pub mod document;` (runner module added in Task 2, command surface in Task 3). Add `pub mod test;` to `commands/mod.rs`.
2. In `document.rs` define, all structs with `#[serde(deny_unknown_fields)]` and `#[serde(rename_all = "camelCase")]` (YAML fields are camelCase: `routeFiles`, `minCount`, …):
   - `pub struct TestDocument { route_files: Option<Vec<String>>, routes: Option<serde_yaml::Value>, inputs: Vec<TestInput>, expects: std::collections::BTreeMap<String, ExpectSet>, settle: Option<String>, settle_parsed: Option<std::time::Duration> }` — `#[serde(default)]` on BOTH `inputs` AND `expects` (missing `expects` must deserialize to empty and reach validation, not fail in serde); `settle` is a humantime string (e.g. `"500ms"`); `settle_parsed` is `#[serde(skip)]`, populated during validation; accessor `pub fn settle_duration(&self) -> Option<std::time::Duration>` returns it.
   - `pub struct TestInput { to: String, #[serde(default, deserialize_with = "deserialize_option_input_body")] body: Option<InputBody>, headers: Option<std::collections::HashMap<String, serde_json::Value>> }` — all optional except `to`. The FIELD-LEVEL custom deserializer is required because serde's default `Option` handling treats an explicit `body: null` identically to a MISSING field (None) — the spec demands null be REJECTED. The helper `fn deserialize_option_input_body<'de, D>(d: D) -> Result<Option<InputBody>, D::Error>` deserializes a PLAIN `serde_json::Value` (NOT `Option<Value>` — serde's Option handling itself swallows explicit null as None): a MISSING field never reaches the helper (`#[serde(default)]` supplies `None`); a PRESENT `body: null` reaches it as `Value::Null` ⇒ `Err(D::Error::custom("unsupported body scalar: null"))` — same sentinel protocol; any other present value ⇒ the InputBody match (String ⇒ `Some(Text)`, Object|Array ⇒ `Some(Json)`, Bool|Number ⇒ sentinel with raw).
   - `pub enum InputBody { Text(String), Json(serde_json::Value) }` with a CUSTOM `impl<'de> serde::Deserialize` using a SENTINEL protocol (untagged cannot work — `serde_json::Value` would accept null/bool/number into the Json arm; and `Deserialize` can only return `D::Error`, not `TestDocError`): deserialize the field as `serde_json::Value` first, then match: `Value::String(s)` ⇒ `Text(s)`; `Value::Object(_) | Value::Array(_)` ⇒ `Json(v)`; `Value::Null | Bool(_) | Number(_)` ⇒ `Err(D::Error::custom(format!("unsupported body scalar: {raw}")))` where raw is the compact serde_json rendering — the string prefix `unsupported body scalar: ` IS the protocol.
   - `pub struct ExpectSet { count: Option<usize>, min_count: Option<usize>, bodies: Option<Vec<String>>, headers: Option<std::collections::HashMap<String, serde_json::Value>> }`; non-string `bodies` entries fail deserialization.
   (Use the `serde_yaml`-compat aliases from noyalib exactly as `crates/camel-dsl/src/yaml.rs` imports them.)
3. Define `#[derive(Debug)] pub enum TestDocError` with a `Display` impl; variants: `Yaml(String)` (wrap the YAML parse error text), `UnknownField(String)`, `RouteSourceConflict`, `ExpectsEmpty`, `ExpectKeyMissingScheme { key: String }`, `CountAndMinCount(String endpoint)`, `SettleOutOfRange(String raw)`, `UnsupportedInputScheme { target: String }`, `UnsupportedBodyScalar(String raw)`. Implement `std::error::Error` with `source()` None. NOTE: verify noyalib's compat error message for `deny_unknown_fields` rejections empirically while implementing `UnknownField` detection (match on the actual message text; if the compat layer does not surface "unknown field", fall back to classifying every structured-reject as `UnknownField` with the raw message).
4. Add `pub fn parse_test_document(text: &str) -> Result<TestDocument, TestDocError>`: noyalib `from_str::<TestDocument>` mapping errors to `Yaml`/`UnknownField`; then validations in this order: (a) `route_files.is_some() == routes.is_some()` ⇒ `RouteSourceConflict`; (b) `expects.is_empty()` ⇒ `ExpectsEmpty`; (c) each expects key MUST start with `"mock:"` ⇒ else `ExpectKeyMissingScheme { key }`; normalize: rebuild the map with the `mock:` prefix stripped, so downstream code (runner, output lines) uses BARE endpoint names while documents key by URI (`mock:result`), exactly as the blessed spec scenarios write them; (d) any entry with both `count` and `min_count` ⇒ `CountAndMinCount(name)`; (e) `settle` present: `humantime::parse_duration(raw)` error, or parsed value not (`> 0ms` and `<= 5000ms`) ⇒ `SettleOutOfRange(raw)`; store parsed in `settle_parsed`; (f) any input `to` not starting with `"direct:"` ⇒ `UnsupportedInputScheme`. `parse_test_document` (step 4) MUST extract the sentinel BEFORE generic classification: a noyalib/serde error whose text contains `unsupported body scalar: ` maps to `UnsupportedBodyScalar(raw-after-prefix)` ahead of the `Yaml`/`UnknownField` mapping; all other error text falls through to the generic mapping.
5. Unit tests inline in `document.rs` (`#[cfg(test)] mod tests`).

**Tests:** (`cargo test -p camel-cli --lib` — tests under `commands::test::document`)
- `valid_reference_doc_parses`: setup: YAML with `routeFiles: [config/routes.yaml]`, one expects entry keyed `mock:result` `{count: 3}`. action: `parse_test_document`. assert: Ok, expects key normalized to `"result"` (prefix stripped), `count == Some(3)`.
- `valid_inline_routes_parses`: `routes:` list value + expects `{mock:result: {minCount: 2}}`; assert `min_count == Some(2)`, `route_files.is_none()`.
- `unknown_field_rejected`: doc with `bogus: 1`; assert `Err` matching `UnknownField`, message contains the field name or the raw reject text.
- `empty_expects_rejected`: `expects: {}` AND a doc with no `expects` key at all; both assert `ExpectsEmpty` (serde default lets both reach validation).
- `bare_expect_key_rejected`: `expects: {result: {count: 1}}` (no `mock:` prefix); assert `ExpectKeyMissingScheme { key: "result" }`.
- `route_source_conflict_rejected`: both `routeFiles` and `routes`; assert `RouteSourceConflict`.
- `count_and_min_count_rejected`: entry with both; assert `CountAndMinCount("result")`.
- `settle_zero_rejected` and `settle_over_5s_rejected`: `settle: "0ms"` / `settle: "10s"`; assert `SettleOutOfRange`. Boundary sanity inside a passing test: `"1ms"` and `"5s"` parse successfully and `settle_duration()` returns them.
- `non_direct_input_rejected`: `inputs: [{to: seda:q, body: x}]`; assert `UnsupportedInputScheme`.
- `body_scalars_rejected`: null body, `true` body, `7` body — three asserts of `UnsupportedBodyScalar` (or the compat layer's scalar rejection surfaced as that variant).
- `body_forms_accepted`: `"x"` ⇒ `InputBody::Text`; `{a: 1}` ⇒ `InputBody::Json`; `[1,2]` ⇒ `InputBody::Json`.

**Acceptance:**
- `cargo test -p camel-cli --lib` green — and `cargo test -p camel-cli --lib commands::test` runs ≥ 10 tests (module is compiled and reached; guards the C1 false-green class).
- `cargo clippy -p camel-cli -- -D warnings` exits 0; `cargo fmt --check` clean.
- Settle boundary: 1ms and 5000ms both VALID (only 0 and >5000 rejected).

- [x] 1

### Task 2: in-process runner (boot, inputs, settle, evaluate)

**Files:**
- `crates/camel-cli/src/commands/test/runner.rs` (new — `commands/test.rs` gains `pub mod runner;`)
- `crates/camel-cli/Cargo.toml` (modified — add ONLY `tower = { workspace = true, features = ["util"] }`; camel-component-{direct,timer,log,seda,mock}, camel-dsl, camel-core, camel-api, camel-component-api, tokio, noyalib, humantime already present from Task 1 or existing deps)

**Steps:**
1. Constants in `runner.rs`: `pub(crate) const SETTLE_DEADLINE: Duration = Duration::from_secs(5);`, `SAMPLE_INTERVAL: Duration = Duration::from_millis(50);`, `DEFAULT_QUIET: Duration = Duration::from_millis(250);`.
2. `pub struct EndpointResult { pub endpoint: String, pub outcome: Result<(), String> }` and `pub struct TestDocResult { pub endpoint_results: Vec<EndpointResult>, pub doc_error: Option<String> }`.
3. `pub async fn run_test_doc(doc: &TestDocument, doc_dir: &std::path::Path) -> TestDocResult`:
   a. Boot: `let mock = MockComponent::new(); let mut ctx = CamelContext::builder().build().await` — build failure ⇒ return `TestDocResult { endpoint_results: vec![], doc_error: Some(...) }` (nothing to stop). Register `mock.clone()` + `DirectComponent`, `TimerComponent`, `LogComponent`, `SedaComponent` defaults (mirror camel-test `build_context`, harness.rs). Wrap: `let ctx: Arc<tokio::sync::Mutex<CamelContext>> = Arc::new(tokio::sync::Mutex::new(ctx));`. From here on, EVERY exit path ends with the mandatory stop of step g — structure as: `let result = { /* steps b-f as a block */ };` then run the stop, then `return result`.
   b. Routes: `routeFiles` ⇒ `camel_dsl::load_from_file(&doc_dir.join(p))` per path (design D4 — it brings the 16 MiB size cap and path-annotated errors); inline `routes` ⇒ noyalib `to_string(&value)` then `camel_dsl::parse_yaml(&text)`. Any failure ⇒ `TestDocResult { endpoint_results: vec![], doc_error: Some(<path + error text>) }` (stop still runs).
   c. `guard.add_route_definition(def).await` per definition (guard = `ctx.lock().await`); `guard.start().await`; failures ⇒ `doc_error` (stop still runs). Immediately after a successful `start()`, capture `let route_started_at = Instant::now();` — the settle deadline anchors HERE (route execution begin), not when settling starts.
   d. Inputs: per `TestInput`, the camel-test `send_await_reply` pattern (integration_test.rs:359-392) with `use tower::ServiceExt;` for `producer.oneshot()`: lock ctx, `ctx.producer_context()`, `ctx.registry().get("direct")`, `component.create_endpoint(&input.to, &*guard)`, `endpoint.create_producer(Arc::new(camel_component_api::NoOpComponentContext), &producer_ctx)` (an `Arc<dyn RuntimeObservability>`); build the exchange as `Exchange::new(Message::new(body))` where body is `Body::Text(s)` / `Body::Json(v)` from `InputBody` (`Message::new` takes `impl Into<Body>`), then `.set_header(k, v.clone())` per header on the message; `producer.oneshot(exchange).await` with the startup-race retry loop (20ms sleep, 1s deadline). Non-race `Err` ⇒ `doc_error` (stop still runs).
   e. Settle: `let quiet = doc.settle_duration().unwrap_or(DEFAULT_QUIET);` loop: deadline = `route_started_at + quiet + SETTLE_DEADLINE` (spec anchors the budget at route-execution begin; the ratified clarification budgets one full quiet window on top so a valid `settle: "5s"` can never race its own deadline); every `SAMPLE_INTERVAL` sample ALL expects keys' `received_count().await` via `mock.get_endpoint(name)` (names are already BARE — T1 normalization stripped `mock:`); track last-change instant; any count change resets it; exit loop when `now - last_change >= quiet`; if `now > deadline` first ⇒ append `EndpointResult { endpoint: "<settle>", outcome: Err("settle timeout: traffic did not quiesce within the 5s instability budget") }`, skip evaluation (stop still runs). Endpoints absent from registry at sample time count as 0 (route may create them late).
   f. Evaluate: per expects entry (bare name): `mock.get_endpoint(name)` — `None` ⇒ `EndpointResult { endpoint: name, outcome: Err(format!("endpoint '{name}' not created by any route")) }`. `Some(inner)` ⇒ set expectations (`count ⇒ inner.expect_count(n)`, `min_count ⇒ expect_minimum_count(m)`, `bodies ⇒ inner.expect_body(Body::Text(s))` in order, `headers ⇒ inner.expect_header(k, v)`), then `inner.try_assert_satisfied().await` mapping `Ok(())` / `Err(e) ⇒ Err(e.to_string())`.
   g. Stop (mandatory, every exit path after step a's successful build): `{ let mut guard = ctx.lock().await; let _ = guard.stop().await; }` — ignore errors (mirror camel-test `TestGuard` — prevents doc N's live timers polluting doc N+1 in the multi-doc driver).
4. Integration tests in `crates/camel-cli/tests/test_runner.rs` (new file), `#[tokio::test(flavor = "multi_thread")]`, writing temp docs via `std::env::temp_dir()` + unique subdir per test. Document YAML in tests keys `expects` by `mock:`-prefixed URIs (e.g. `mock:result`), asserting `EndpointResult.endpoint` carries the BARE name (`result`).

**Tests:** (`cargo test -p camel-cli --test test_runner`)
- `timer_route_settles_and_passes`: setup: inline doc, route `from: timer:tick?period=50&repeatCount=3` → `to: mock:result`, expects `{mock:result: {count: 3}}`. action: `run_test_doc`. assert: `doc_error.is_none()`, one `EndpointResult` for bare `result` with `outcome Ok(())`. (~0.5s)
- `direct_input_reaches_mock`: route `from: direct:start` → `to: mock:out`, input `{to: direct:start, body: "x"}`, expects `{mock:out: {count: 1, bodies: ["x"]}}`; assert Ok. `bodies` ordered assert proves delivery.
- `route_files_resolve_relative_to_doc_dir`: setup: tempdir `t/` containing `cfg/demo.yaml` (route direct:start → mock:rel) and `t/doc.test.yaml` with `routeFiles: [cfg/demo.yaml]`, input, expects `{mock:rel: {count: 1}}`. action: `run_test_doc(&doc, t/)`. assert: Ok — proves `doc_dir.join` resolution via `load_from_file`.
- `mismatch_reports_change1_detail`: expects `{mock:result: {count: 3}}`, input-driven route producing 2; assert `outcome Err` whose text contains `"expected 3 exchanges, got 2"`.
- `absent_endpoint_fails`: expects `{mock:ghost: {count: 1}}` with route creating only `mock:result`; assert Err text contains `"ghost"`.
- `failure_does_not_abort_other_endpoints`: two mock sinks, one expectation fails, other passes; assert both `EndpointResult`s present, one Ok one Err.
- `settle_window_resets_on_change`: timer period 100ms repeatCount 4, quiet 250ms. camel-timer fires the FIRST tick immediately (delay default 0), so ticks land at ~0/100/200/300ms and settle exits ≈550-600ms; assert overall Ok and `elapsed >= 500ms` (lower bound proves a reset occurred; do NOT assert ≥650ms — first-fire-immediate makes that flaky-false).
- `settle_deadline_times_out`: timer period 10ms repeatCount 100000 (emits far past deadline); assert the `"<settle>"` EndpointResult with text containing `"settle timeout"`, `doc_error.is_none()`. (Runs ~5s — annotate `// slow: deadline cap`.)

**Acceptance:**
- `cargo test -p camel-cli --test test_runner` green; `--lib` still green.
- `cargo clippy -p camel-cli --all-targets -- -D warnings` exits 0; `cargo fmt --check` clean; `cargo xtask lint-unwrap` clean (tests follow the `// allow-unwrap` convention where needed).

- [x] 2

### Task 3: CLI wiring — `camel test` command + multi-document driver

**Files:**
- `crates/camel-cli/src/commands/test.rs` (modified — add command surface to the existing module root)
- `crates/camel-cli/src/main.rs` (modified — Commands enum + match arm)

**Steps:**
1. In `commands/test.rs`: `#[derive(clap::Args)] pub struct TestArgs { #[arg(value_name = "FILE", required = true)] pub files: Vec<std::path::PathBuf> }` — `required = true` is load-bearing (a bare `Vec` positional is empty-OK otherwise); derive style matches `LintArgs`.
2. `pub struct TestRunSummary { pub exit_code: i32, pub passed: usize, pub failed: usize }` and `pub async fn run_tests(files: &[std::path::PathBuf], out: &mut dyn std::io::Write, err: &mut dyn std::io::Write) -> TestRunSummary`:
   - For each file in CLI argument order: read file (read failure ⇒ `writeln!(err, ...)`, record parse-error class, continue), `parse_test_document` (failure ⇒ `writeln!(err, "{path}: {error}")`, continue), `run_test_doc(&doc, parent_dir)`.
   - Print per endpoint: `writeln!(out, "PASS {doc}#{endpoint}")` or `writeln!(out, "FAIL {doc}#{endpoint} — {detail}")`.
   - `doc_error` ⇒ `writeln!(err, ...)`; counts as parse-error class.
   - Summary line: `writeln!(out, "{passed} passed, {failed} failed")`.
   - Exit precedence: any parse-error class ⇒ 2, else any failed ⇒ 1, else 0.
3. `main.rs`: add `/// Run declarative mock tests from *.test.yaml documents.` + trust-model line mirroring `Run`'s doc comment; `Test(commands::test::TestArgs)` variant; match arm: build stdout/stdout-lock + stderr-lock writers, `let summary = commands::test::run_tests(&files, &mut out, &mut err).await; std::process::exit(summary.exit_code);` (match existing main.rs style).
4. Unit tests in `commands/test.rs` (`#[cfg(test)]`) using `Vec<u8>` writers for both `out` and `err`, driving `run_tests` with temp docs. All doc fixtures key `expects` by `mock:`-prefixed URIs (e.g. `mock:result`) — bare keys are a parse error per Task 1.

**Tests:** (`cargo test -p camel-cli --lib commands::test` filter)
- `all_pass_exits_zero`: one passing doc; assert `exit_code == 0`, `out` contains `"PASS"` line and `"1 passed, 0 failed"`.
- `assertion_failure_exits_one`: one failing expectation; assert `exit_code == 1`, `"FAIL"` line in `out`, `"0 passed, 1 failed"`.
- `parse_error_continues_and_exits_two`: arg order `[a(passing), bad(invalid YAML)]`; assert a's PASS line in `out` (both attempted), `bad` path + error text in `err`, `exit_code == 2`.
- `precedence_parse_beats_assertion`: `[a(failing expectation), bad(invalid)]`; assert `exit_code == 2`.
- `multi_doc_second_failing_both_evaluated`: `[a(passing), b(one failing expectation)]`; assert both docs' endpoint lines in `out`, `exit_code == 1` (spec scenario combo).
- `multi_doc_arg_order`: two passing docs; assert PASS lines appear in arg order (index comparison in `out`).
- `missing_file_exits_two`: nonexistent path; assert exit 2, `err` names the path.

**Acceptance:**
- `cargo test -p camel-cli --lib` green; `--test test_runner` green.
- `cargo clippy -p camel-cli --all-targets -- -D warnings` exits 0; `cargo fmt --check` clean.
- Manual smoke (report only): `cargo run -p camel-cli -- test <temp passing doc>` exits 0.

- [x] 3

### Task 4: `camel run` default-glob exclusion of `*.test.yaml`

**Files:**
- `crates/camel-cli/src/commands/run.rs` (modified)
- `crates/camel-cli/Cargo.toml` (modified — add `glob` to deps and `tempfile` to dev-deps if absent)

**Steps:**
1. Add helpers in `run.rs`:
   - `fn expand_patterns_excluding_test_docs(patterns: &[String]) -> Vec<String>` — for each pattern, expand with the `glob` crate; collect matched paths; filter out any whose file name ends with `.test.yaml` or `.test.yml`; return surviving paths as `String`s (literal paths are valid glob inputs to discovery). On glob error, pass the original pattern through unchanged (fail-open to existing behavior).
   - `fn default_patterns() -> Vec<String>` — the existing default `vec!["routes/*.yaml".to_string()]` construction (~run.rs:241), extracted as a fn.
2. Rewrite the THREE-WAY pattern computation (~run.rs:234-243) with an injectable-defaults core:
   - `fn resolve_route_patterns_with(defaults: &[String], routes_override: &Option<String>, config_routes: &Option<Vec<String>>) -> Vec<String>`: `Some(ov)` ⇒ `vec![ov.clone()]` untouched; `None` + config routes present ⇒ `config_routes.clone()` UNTOUCHED (Camel.toml entries pass through — D8 scopes exclusion to the default glob only); `None` + no config ⇒ `expand_patterns_excluding_test_docs(defaults)`.
   - `fn resolve_route_patterns(routes_override: &Option<String>, config_routes: &Option<Vec<String>>) -> Vec<String>` delegates with `default_patterns()` — the production entry.
   Wiring, three concerns kept separate: (i) the INITIAL load (~run.rs:453 consumer) calls `resolve_route_patterns` once. (ii) The WATCH reload closure (~run.rs:521-534) must NOT capture a frozen expanded path list — it captures the resolver INPUTS and re-invokes `resolve_route_patterns` on EVERY reload pass, so each pass re-globs (picks up new/deleted files) and re-filters test documents per pass. (iii) Watch DIRECTORY derivation (`watch_dirs` ~run.rs:523,529) must consume the RAW, UNEXPANDED patterns — the same three-way selection WITHOUT expansion/filtering (extract as `fn raw_route_patterns(routes_override, config_routes) -> Vec<String>` if the inline form is awkward): an initially-empty routes dir must still yield `routes/` as a watched root so newly created files trigger reload. Never derive watch dirs from the filtered expansion (empty expansion ⇒ no watched root ⇒ watcher dead).
3. Unit tests inline in `run.rs` test module (create if absent) using `tempfile`.

**Tests:** (`cargo test -p camel-cli --lib resolve_route_patterns` and expansion filter)
- `test_docs_excluded_from_default_expansion`: setup: tempdir with `routes/demo.yaml` (minimal valid route text: `- from: {uri: direct:x, steps: [{to: {uri: mock:m}}]}`) and `routes/demo.test.yaml` (any bytes). action: `expand_patterns_excluding_test_docs(&[tempdir + "/routes/*.yaml"])`. assert: result == vec![path to demo.yaml].
- `yml_suffix_also_excluded`: same with `demo.test.yml`; assert excluded.
- `override_passthrough_untouched`: `resolve_route_patterns(&Some("routes/*.test.yaml".into()), &None)` returns exactly `vec!["routes/*.test.yaml"]` (unexpanded, unfiltered) — explicit `--routes` override honored.
- `config_routes_passthrough_untouched`: `resolve_route_patterns(&None, &Some(vec!["custom/*.yaml".into()]))` returns the config entries verbatim — Camel.toml routes never filtered.
- `none_returns_expanded_defaults`: `resolve_route_patterns_with(&[format!("{tempdir}/routes/*.yaml")], &None, &None)` (injectable defaults make the fixture hermetic) returns only `demo.yaml`'s path.
- `raw_patterns_stay_unexpanded_for_watch_dirs`: `raw_route_patterns(&None, &None)` returns exactly `vec!["routes/*.yaml".to_string()]` (unexpanded, unfiltered) — watch-dir derivation keeps glob semantics; also `raw_route_patterns(&Some("custom/*.yaml".into()), &None)` returns the override verbatim.
- `non_matching_pattern_returns_empty`: expansion of a pattern matching nothing ⇒ empty vec, no error.

**Acceptance:**
- `cargo test -p camel-cli --lib` green.
- `cargo clippy -p camel-cli -- -D warnings` exits 0; `cargo fmt --check` clean.
- `discover_routes_with_threshold_and_security` still called from exactly the two existing sites; the watch closure re-resolves patterns per reload (grep: no frozen `patterns.clone()` captured as the glob source — the clone is replaced by per-pass `resolve_route_patterns` invocation).

- [x] 4

### Task 5: docs, example test document, gate sweep

**Files:**
- `crates/camel-cli/README.md` (modified)
- `examples/yaml-dsl/config/mock-demo.test.yaml` (new)
- `examples/yaml-dsl/config/mock-demo.yaml` (new — small companion route file: `from: direct:start` → one transform step → `to: mock:result`)

**Steps:**
1. README section "camel test": document format (routeFiles XOR routes, inputs direct-only, expects fields keyed by `mock:`-prefixed endpoint URI — `mock:result` — with the runner normalizing to the bare name, settle as humantime string), exit codes 0/1/2, precedence 2 > 1 > 0, non-interference note (default run globs skip `*.test.yaml` per discovery pass — new route files still load on watch reload; explicit `--routes` and Camel.toml routes pass through untouched).
2. Example pair: `mock-demo.yaml` route + `mock-demo.test.yaml` (routeFiles reference `mock-demo.yaml` — relative to the test doc's directory, one input body, expects count+bodies keyed `mock:result`). Verify by hand-run: `cargo run -p camel-cli -- test examples/yaml-dsl/config/mock-demo.test.yaml` exits 0 (record output in report).
3. Gate sweep from worktree root, all exit 0: `cargo fmt --check --all`; `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings`; `cargo clippy -p camel-cli -- -D warnings`; `cargo test -p camel-cli --lib`; `cargo test -p camel-cli --test test_runner`; `cargo xtask lint-unwrap`; `cargo xtask lint-secrets`; `cargo xtask lint-log-levels`; `cargo xtask lint-non-exhaustive`; `cargo xtask lint-ignore`; `cargo xtask lint-component-deps`; `cargo xtask schema --check`.
4. Fix anything surfaced (docs wording, fmt drift) — zero functional changes expected.

**Tests:**
- `readme_documents_test_command`: report-only check — `grep -c 'camel test' crates/camel-cli/README.md` ≥ 1 and exit codes documented.
- `example_doc_runs_green`: the step-2 hand-run IS the test (exit 0 recorded in report).

**Acceptance:**
- All 12 sweep commands exit 0 (record codes in the report).
- Example pair committed; README renders.

- [x] 5
