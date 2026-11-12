# Tasks: business-error-model

Owner decisions 2026-11-12 (e_opus review): Q1 mapping on a separate route-level field (never `ErrorHandlerConfig`; rest keeps `error_handler: None`; rc-7lkp2 Ok-with-error hole covered), Q2 `${exception.kind}` = `classify()` with `rc-7qv90` as prerequisite + payload under `${exception.business_kind}`, Q3 kebab-case kinds (`invalid-argument`, `conflict`, `not-found`, `forbidden`, `failed`), Q4 business errors count as breaker SUCCESS, Q5 plain-route YAML mapping deferred to phase 3, Q6 `business_kind` on `on_exceptions` deferred to phase 3, Q7 mapped >=500 logs warn / <500 info. Findings applied: C1 (rc-7lkp2 coverage), I1+I2 (redacting Debug newtype; send-to-handler info carve-out), I3 (MODIFIED deltas for camel-http and the dsl wildcard requirement, reproducing all canonical scenarios), M1 (constructor contract in rustdoc+ADR), M2 (broker redelivery note).

## Phase 1: core business error model

Raise, catch, expose, and enforce policy for `CamelError::Business` without HTTP. Matches `design.md ## Phases` Phase 1. Prerequisite: bd `rc-7qv90` (`${exception.kind}` parse/evaluate as the `classify()` name) MUST land before task 1.5 — owner-set ordering (Q2); it is not implemented here.

### Task 1.1: Business variant, RedactedJson newtype, classify/variant_name, vocabulary

**Files:**
- `crates/camel-api/src/error.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)

**Steps:**
1. Add variant `Business { code: String, kind: String, message: String, data: Option<RedactedJson> }` to `CamelError` with a Display that shows `code` and `kind` but never `message` internals beyond the summary and never `data`.
2. Define `RedactedJson` as a newtype over `serde_json::Value` in camel-api: `Debug` prints `[redacted]` (never the payload); `PartialEq`/`Eq`/`Clone` derive through the inner value; constructors `new(Value)` and accessor `as_value(&self) -> &Value` for the Simple evaluator and internal consumers (review m1).
3. Document the constructor contract on the variant's rustdoc (M1): non-blank `code`/`kind` validated at builders' boundaries, `message` is log-only and never reaches a client, `data` is redacted diagnostics.
4. Extend `classify()` (`error.rs:484`): the `Business` arm returns `"business"`. Extend `variant_name()` (`error.rs:526`): returns `"Business"`.
5. Add `"Business"` to `supported_exception_kinds()` (`compile.rs:922`) and to the classification test table near `compile.rs:2661`/`:2732` as matchable under the `Business` kind value.

**Tests:**
- `classify_and_variant_name_business` (in-file tests, `error.rs`): construct `Business { code: "ORDER_NOT_FOUND", kind: "not-found", message: "m", data: None }` → assert `classify() == "business"` and `variant_name() == "Business"`.
- `debug_on_data_newtype_redacts` (`error.rs` in-file): `Business` with `data = {"secret": "hunter2"}` formatted with `{:?}` → output contains `[redacted]` and does not contain `hunter2`; `PartialEq` on two equal `RedactedJson` values still holds.
- `vocabulary_accepts_business_kind` (`compile.rs` in-file near `:2661`): expected vocabulary vector includes `"Business"` → assert `supported_exception_kinds()` contains it.
- `classification_table_pins_business_variant` (`compile.rs` in-file near `:2732`): the guard table classifies `Business` as matchable under `"Business"` → guard passes with the entry, fails without it (remove-then-restore during development).

**Test command:** `cargo test -p camel-api --lib` and `cargo test -p camel-dsl --lib`
**Expected before implementation:** new tests fail to compile (variant/newtype missing) — RED.

**Acceptance:**
- `cargo test -p camel-api --lib` passes; `cargo test -p camel-dsl --lib` passes.
- `cargo clippy -p camel-api -- -D warnings` and `cargo clippy -p camel-dsl -- -D warnings` exit 0.
- `cargo fmt --check` clean.

- [x] 1.1

### Task 1.2: throw step (model, AST, contract, core compiler, compile, load validation)

**Files:**
- `crates/camel-dsl/src/model.rs` (modified)
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-dsl/src/contract.rs` (modified)
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified)
- `crates/camel-dsl/tests/throw_step_tests.rs` (new)

**Steps:**
1. Add `ThrowStepDef { code: String, kind: Option<String>, message: Option<ValueSourceDef>, data: Option<serde_json::Value> }` to `model.rs` and `DeclarativeStep::Throw(ThrowStepDef)`. `data` string leaves hold Simple templates at authoring time.
2. Add `ThrowStep { throw: ThrowData }` to `route_ast.rs` (`ThrowData` with `deny_unknown_fields`, schema/ts derives like sibling steps) and the `RouteDslStep::Throw` variant; translate to `DeclarativeStep::Throw` in `yaml.rs`.
3. Load-time validation in the shared translation path (`route_dsl_to_declarative_route`, exercised by both YAML and JSON formats): blank or missing `code` (trim-based) and explicitly blank `kind` produce typed `CamelError::RouteError` naming the field; `data` that is not a JSON object (string or array included) is rejected the same way. No JSON-specific duplication — one validation, two formats.
4. Register the `throw` step kind in `contract.rs` (step-kind inventory and its coverage test gain the new kind).
5. Compilation follows the `set_header` precedent: `compile.rs` maps `DeclarativeStep::Throw` to the late-bound carrier `BuilderStep::DeclarativeThrow { code, kind, message, data }` (mirroring `BuilderStep::DeclarativeSetHeader { key, value }` from `compile_set_header_step`), defined in `camel-core/src/lifecycle/application/route_definition.rs`, and the core step compiler (`camel-core/src/lifecycle/adapters/step_compilers/core.rs`) resolves it — evaluates the `message`/`data` templates through the same Simple path at runtime and returns `Err(CamelError::Business { code, kind: kind.unwrap_or("failed"), message, data: data.map(RedactedJson::new) })`. The raise-site debug log (task 1.6 step 1) lives in this compiled throw processor. A template evaluation failure returns the typed evaluation error per `language-value-boundary`; `data` is never written to the body or to any exchange property.

**Tests:** (in `crates/camel-dsl/tests/throw_step_tests.rs`)
- `parse_throw_yaml`: YAML route with a `throw` step (code, kind, message template, data object) loads → `DeclarativeStep::Throw` present with the literal `code`/`kind` and template `message`.
- `blank_code_fails_load`: `- throw: { code: "  " }` → load fails with a typed error naming `code`.
- `missing_code_fails_load`: `throw` step without `code` → load fails naming `code`.
- `blank_kind_fails_load`: `kind: " "` → load fails naming `kind`.
- `non_object_data_fails_load`: `data` as a string, and in a second document `data` as an array → each load fails naming `data`.
- `json_format_compiles_and_raises_throw`: equivalent JSON route compiles → same `ThrowStepDef`; executing the JSON-compiled step raises the same `Business` error at runtime.
- `throw_raises_business_with_templates`: exchange with header `id: "A-1"`; step `{ code: "ORDER_NOT_FOUND", kind: "not-found", message: "Order ${header.id} not found", data: { orderId: "${header.id}" } }` executes → step result is `Err(Business)` with all four fields resolved, `data == {"orderId":"A-1"}`.
- `throw_defaults_kind_and_message`: step `{ code: "STOCK_LOW" }` → error carries `kind: "failed"` and `message: "STOCK_LOW"`.
- `throw_data_nested_quotes_and_nonstring`: data `{ order: { id: "${header.id}", note: "say \"hi\"" }, attempts: 3, priority: true }` with `id: "A\"1"` → resolved data equals the nested document with the quoted id, the constant quoted leaf, and the non-string leaves preserved.
- `throw_int_coercion_failure_is_runtime_error`: data leaf `"${int(header.absent)}"` with no header → step fails with a typed evaluation error, not `Business`; route still loaded. A sibling leaf `"${header.absent}"` resolves to null (canonical absent-header rule) and does not fail.
- `throw_writes_no_body_or_property`: after a raised throw, exchange body and properties are unchanged.

**Test command:** `cargo test -p camel-dsl --test throw_step_tests`
**Expected before implementation:** file does not compile / tests fail — RED.

**Acceptance:**
- All task tests pass; `cargo test -p camel-dsl --lib` still passes.
- `cargo xtask schema --check` exits 0 (schema/ts derives updated with the new step).
- `cargo clippy -p camel-dsl -- -D warnings` and `cargo clippy -p camel-core -- -D warnings` exit 0.
- `cargo fmt --check` clean.

- [x] 1.2

### Task 1.3: doTry catch business matchers (code, business_kind, on_when)

**Files:**
- `crates/camel-dsl/src/model.rs` (modified)
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/control_flow.rs` (modified)
- `crates/camel-processor/src/do_try.rs` (modified)
- `crates/camel-processor/src/do_try_segment.rs` (modified)
- `crates/camel-test/tests/do_try_test.rs` (modified)

**Steps:**
1. Add `code: Option<String>` and `business_kind: Option<String>` to `CatchClauseData` (`route_ast.rs:923`) and `DoTryCatchClauseDef` (`model.rs:717`), with `deny_unknown_fields` semantics inherited from the struct.
2. Extend the load-time matcher validation in `yaml.rs` (currently `yaml.rs:1838` exclusivity and `yaml.rs:1847` `on_when` rule): main matcher set becomes `exception` XOR `when` XOR `code` XOR `business_kind` (typed error on any pair); `on_when` is allowed only with `exception`, `code`, or `business_kind` as the main matcher.
3. Compile the new matchers through the clause builder and core control-flow compiler: `DoTryCatchClauseBuilder` (`camel-core/src/lifecycle/application/route_definition.rs:45`, referenced as `catch: Vec<DoTryCatchClauseBuilder>` at `:389`) gains the `code`/`business_kind` fields; `camel-core/src/lifecycle/adapters/step_compilers/control_flow.rs` passes them into the compiled segment, so `code` matches `Business` payload equality, `business_kind` matches `Business.kind`, and any non-`Business` error never matches either.
4. Runtime matching in `camel-processor/src/do_try.rs` and `do_try_segment.rs` evaluates the extended matcher; first-match-wins order is unchanged; the catch-body-failure envelope is untouched.

**Tests:** (in `crates/camel-test/tests/do_try_test.rs`, following its existing route-test style)
- `catch_by_code_matches_business_error`: doTry body throws `Business { code: "ORDER_NOT_FOUND" }`; catch `{ code: "ORDER_NOT_FOUND", disposition: handled }` → outcome `Completed`, catch body ran.
- `catch_by_code_with_on_when_combination`: clauses `[{code: "ORDER_NOT_FOUND", on_when: "${header.retry} == 'true'", disposition: handled}, {code: "ORDER_NOT_FOUND", disposition: handled}]`; with `retry: "true"` the first clause runs; with `retry: "false"` the second runs.
- `catch_by_business_kind`: catch `{ business_kind: "conflict", disposition: handled }`; body throws `Business { kind: "conflict" }` → handled; body throws `ValidationError("bad")` → no match, outcome `Failed`.
- `two_payload_matchers_fail_load`: clause with both `code` and `business_kind` → load fails with the typed matcher-exclusivity error.
- `on_when_with_when_alone_fails_load`: clause with `when` + `on_when` → load fails with the existing `on_when` rule error.
- `non_business_never_matches_code_clause`: catch `{ code: "X" }`; body throws `Io("disk")` → no match.

**Test command:** `cargo test -p camel-test --test do_try_test` and `cargo test -p camel-dsl --lib`
**Expected before implementation:** new tests fail (unknown fields rejected by `deny_unknown_fields`) — RED.

**Acceptance:**
- All task tests pass; existing `do_try_test` cases stay green.
- `cargo clippy -p camel-dsl -- -D warnings`, `cargo clippy -p camel-core -- -D warnings`, and `cargo clippy -p camel-processor -- -D warnings` exit 0.
- `cargo fmt --check` clean.

- [x] 1.3

### Task 1.4: on_exceptions code matching

**Files:**
- `crates/camel-dsl/src/model.rs` (modified)
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-dsl/tests/on_exception_code_tests.rs` (new)

**Steps:**
1. Add `code: Option<String>` to `DeclarativeOnException` (`model.rs:219`) and to `RouteDslOnException` (`route_ast.rs:403`).
2. Carry `code` through `RouteDslOnException` into the shared `route_dsl_to_declarative_route` translation (the `yaml.rs:467` struct literal gains the field); YAML and JSON formats share this path, so no format-specific work exists.
3. Extend clause matching in `compile.rs`: `code` is one more conjunctive matcher beside `kind` and `message_contains`; it matches only `Business` errors with equal `code`; first-match-wins order unchanged.

**Tests:** (in `crates/camel-dsl/tests/on_exception_code_tests.rs`)
- `code_matches_business_error`: clause `[{code: "ORDER_NOT_FOUND", handled: true}]`; step throws `Business { code: "ORDER_NOT_FOUND" }` → clause matches, pipeline `Completed`.
- `code_does_not_match_other_code_or_system_error`: same route; step throws `Business { code: "OTHER_CODE" }` then, in a second exchange, `Io("disk")` → neither matches; the `Io` exchange takes the unmatched-error path.
- `code_conjunction_with_kind`: clause `[{kind: "Business", code: "ORDER_NOT_FOUND", handled: true}]` matches the business error and does not match `Io`.
- `kind_business_matches_only_business_runtime`: clause `[{kind: "Business", handled: true}]` matches `Business` and does not match `ValidationError`.
- `first_match_wins_with_code_clauses`: clauses `[{code: "A", handled: true}, {code: "A", handled: true, steps: [marker]}]` → only the first clause's effect is visible.

**Test command:** `cargo test -p camel-dsl --test on_exception_code_tests`
**Expected before implementation:** unknown field `code` rejected — RED.

**Acceptance:**
- All task tests pass; `cargo test -p camel-dsl --lib` passes (vocabulary tests from 1.1 stay green).
- `cargo clippy -p camel-dsl -- -D warnings` exit 0; `cargo fmt --check` clean.

- [x] 1.4

### Task 1.5: Simple exception paths (code, business_kind, data; kind via rc-7qv90)

**Files:**
- `crates/languages/camel-language-simple/src/parser.rs` (modified)
- `crates/languages/camel-language-simple/src/evaluator.rs` (modified)
- `crates/camel-test/tests/do_try_test.rs` (modified)

**Steps:**
1. Add `Expr::ExceptionCode`, `Expr::ExceptionBusinessKind`, and `Expr::ExceptionData(Vec<PathSegment>)` to the parser grammar; parse `${exception.code}`, `${exception.business_kind}`, and `${exception.data.<path>}` where `<path>` is a dot path of keys into the `data` object. Index segments (`PathSegment::Index`) are rejected in `exception.data` paths at parse time — the spec traverses nested objects only, so `${exception.data.items.0}` fails with a typed parse error instead of inheriting body-field array semantics.
2. Evaluate against `exchange.error` (unwrapping `RedactedJson` internally): `ExceptionCode` yields the business `code` when the error is `Business`, else empty; `ExceptionBusinessKind` yields the payload `kind` when `Business`, else empty; `ExceptionData` traverses `data` by the path, yields empty for missing/null/non-traversable fields, and renders non-string leaves per the existing scalar rules (ADR-0092). All new paths yield empty when no error is present.
3. `${exception.kind}` parses and evaluates as the `classify()` name — that support lands with bd `rc-7qv90`, a prerequisite of this task (owner decision Q2). This task adds no `exception.kind` grammar and does not touch rc-7qv90's tests; where rc-7qv90 is not yet merged, this task's branch builds on a stack containing it.
4. Add no exchange properties in the evaluator; the exception properties `Exchange::set_error()` derives keep their existing semantics.

**Tests:** (in-file tests, `parser.rs` and `evaluator.rs`; integration test in `camel-test/tests/do_try_test.rs`)
- `parse_exception_code_business_kind_and_data`: `parse("${exception.code}")` → `Expr::ExceptionCode`; `parse("${exception.business_kind}")` → `Expr::ExceptionBusinessKind`; `parse("${exception.data.order.id}")` → `Expr::ExceptionData([Key("order"), Key("id")])`.
- `parse_exception_data_index_rejected`: `parse("${exception.data.items.0}")` → typed parse error (no Index segment).
- `eval_business_code_payload_kind_and_nested_data`: exchange error `Business { code: "ORDER_NOT_FOUND", kind: "not-found", data: {"order":{"id":"A-1"}} }` → `${exception.code}` is `ORDER_NOT_FOUND`, `${exception.business_kind}` is `not-found`, `${exception.data.order.id}` is `A-1`; `${exception.kind}` (rc-7qv90 surface) is `business`.
- `eval_non_business_code_empty_business_paths`: exchange error `Io("disk")` → `${exception.code}` empty, `${exception.business_kind}` empty; `${exception.kind}` (rc-7qv90 surface) is `io`.
- `eval_no_error_paths_empty`: no error → `${exception.code}`, `${exception.business_kind}`, `${exception.data.any}` empty, evaluation does not fail.
- `eval_null_and_scalar_intermediate_data_empty`: data `{"a": {"b": null}, "n": 3}` → `${exception.data.a.b}` and `${exception.data.a.b.c}` empty; `${exception.data.n}` renders `3`; `${exception.data.n.child}` empty (traversal into a scalar terminates empty).
- `eval_missing_null_untraversable_data_empty`: data `{"a":{"b":1}}` → `${exception.data.zzz}`, `${exception.data.a.c}` empty; `${exception.data.a.b}` renders `1`.
- `catch_visibility_simple_code_inside_catch_body` (`camel-test/tests/do_try_test.rs`): doTry body throws `Business { code: "Y" }`; matching handled catch body evaluates `${exception.code}` into a body marker → marker reads `Y` inside the catch body; a step after the block reads empty (error cleared).

**Test command:** `cargo test -p camel-language-simple --lib` and `cargo test -p camel-test --test do_try_test catch_visibility`
**Expected before implementation:** new parse tests fail (unknown expression) — RED.

**Acceptance:**
- All task tests pass.
- Prerequisite recorded: bd `rc-7qv90` lands BEFORE this task (owner decision Q2); this change does not implement it.
- `cargo clippy -p camel-language-simple -- -D warnings` exit 0; `cargo fmt --check` clean.

- [x] 1.5

### Task 1.6: policy — logs, redaction, no redelivery, DLC bypass, breaker success

**Files:**
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified)
- `crates/camel-processor/src/error_handler.rs` (modified)
- `crates/camel-processor/src/circuit_breaker.rs` (modified)
- `crates/camel-processor/src/do_try_segment.rs` (modified)
- `crates/camel-core/src/shared/observability/adapters/tracer.rs` (modified)
- `crates/camel-core/src/shared/observability/adapters/tracer_tests.rs` (modified)
- `crates/camel-test/tests/business_error_policy_test.rs` (new)

**Steps:**
1. Raise-site logging: the compiled throw processor (`BuilderStep::DeclarativeThrow` resolved in `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs`, task 1.2 step 5) emits a debug-level record carrying `code` and `kind` structured; `data` and `message` are never serialized into the record (the `RedactedJson` Debug makes any accidental structured logging print `[redacted]`).
2. Handling logging: clause disposition in `error_handler.rs`, a handled catch in `do_try_segment.rs`, and the `handled_by`/send-to-handler delegate dispatch all emit info with `code`/`kind` and never `data` (I2 carve-out: the send-to-handler path is its own log site, covered by its own test).
3. Propagate logging is transport-owned and never doubled (review I1). PHASE 1 SCOPE (non-seam routes only — seam fields land in Phase 2): on routes with a handler configured whose policy chain produced no producer (`forward_or_propagate(ex, None, err)`, e.g. an `error_handler` block present but empty) the error handler logs the unmatched business propagate at warn; on routes with NO handler installed (`route_compiler_ext.rs:260-274`, configuration absent) no error-handler log site exists — failure logging is transport-owned (existing), out of scope here. The seam-route side (error handler debug, finalizer info/warn by status) is task 2.2/2.1 work.
4. Redelivery skip in `error_handler.rs`: when the failing error is `Business`, clause-level AND route-level redelivery policies produce zero redeliveries, the step runs once, the matching disposition still runs, and `CamelRedelivered` is not set. Declarative route redelivery only — broker-level client redelivery is a transport concern and is not touched (M2 note, documented in the spec and ADR).
5. Non-seam dead-letter parity in `error_handler.rs` (review C1): on non-seam routes (no mapping field) business errors keep dead-letter dispatch exactly as today (Apache Camel parity: `throwException` lands in the DLC), including the synthetic Handled catch-all path (`route_compiler_ext.rs:77-86`). The SEAM-CONDITIONAL bypass (skip DLC + synthetic policy + matched-clause-without-handled_by fallback on `http_error_mapping: Some(_)`) is Phase 2 work — task 2.2 — because the seam field lands there (phase dependency fix, review round 3). Explicit clauses (handled, handled_by, steps, continued) still catch business errors on every route; non-business errors keep DLC dispatch everywhere.
6. Breaker success in `circuit_breaker.rs`: `CircuitBreakerService` (`:136`) and `CircuitBreakerGate` (`:464`) both record a `Business` error as a SUCCESS in every state; a half-open probe ending in any business error completes as success and the breaker closes (owner decision Q4).
7. Tracer exclusion in `tracer.rs`: business failures skip the error increment the same way the existing `CIRCUIT_OPEN` exclusion does.

**Tests:** (in `crates/camel-test/tests/business_error_policy_test.rs` — this crate already depends on camel-dsl, so DSL-driven route tests live here, not in camel-core; in-file unit tests in `circuit_breaker.rs` and `tracer_tests.rs`)
- `raise_logs_debug_with_code_kind_no_data`: throw with data → exactly one debug record with `code`/`kind` fields, no record contains the data payload.
- `clause_handled_logs_info_no_data`: clause-handled business error (on_exceptions clause, disposition handled) → info record with `code`/`kind`, no `data`.
- `catch_handled_logs_info_no_data`: doTry catch-handled business error → info record with `code`/`kind`, no `data`.
- `send_to_handler_logs_info_no_data`: clause `[{code: "X", handled_by: "direct:shaper"}]` delegates a business error → info record at the delegate dispatch site with `code`/`kind`, no `data` (I2 carve-out).
- `non_seam_route_propagate_logs_warn`: route with an `error_handler` block present but empty (handler installed, policy chain produced no producer); unmatched business error → error handler warn record (`forward_or_propagate(ex, None, err)` site). Routes with NO handler installed have no error-handler log site — their failure logging is transport-owned and out of Phase 1 scope.
- `seda_unmatched_business_error_reaches_dlc`: InOnly seda consumer route with `dead_letter_channel: "mock:dlc"` and no mapping (non-seam) → `mock:dlc` receives the exchange (C1: bypass is seam-conditional; Camel parity).
- `wildcard_matches_business_no_redelivery`: clause `[{kind: "*", handled: true, handled_by: "direct:shaper"}]` matches a business error → shaper output is final, no redelivery (I4: owns the dsl wildcard-business scenario).
- `clause_retry_never_redelivers_business`: clause `[{code: "X", retry: {max_attempts: 3}, handled: true}]`; step throws `Business { code: "X" }` → step executed once, no `CamelRedelivered`, clause disposition ran.
- `route_level_retry_suppressed_for_business`: route-level `error_handler.retry: {max_attempts: 3}` with no matching clause; step throws `Business` → executed once, no redelivery, no `CamelRedelivered`.
- `breaker_service_records_success` and `breaker_gate_records_success` (`circuit_breaker.rs` in-file, threshold 1): business failure → breaker stays closed, recorded as success.
- `half_open_service_closes_on_business` and `half_open_gate_closes_on_business` (`circuit_breaker.rs` in-file): breaker half-open after system failures; probe ends in any business error (handled or propagated) → probe recorded success, breaker closes, for each implementation.
- `tracer_skips_error_increment_for_business` (`tracer_tests.rs`): business failure does not increment the error metric; `Io` failure still does.

**Test command:** `cargo test -p camel-processor --lib` and `cargo test -p camel-test --test business_error_policy_test` and `cargo test -p camel-core --lib tracer`
**Expected before implementation:** new tests fail (business errors currently counted/logged as generic failures) — RED.

**Acceptance:**
- All task tests pass; existing retry/breaker/tracer tests stay green.
- `cargo xtask lint-log-redaction` and `cargo xtask lint-log-levels` exit 0.
- `cargo clippy -p camel-processor -- -D warnings`, `cargo clippy -p camel-core -- -D warnings`, and `cargo clippy -p camel-test -- -D warnings` exit 0.

- [x] 1.6

### Task 1.7: async boundary propagation tests

**Files:**
- `crates/camel-core/tests/business_error_propagation_tests.rs` (new)

**Steps:**
1. Direct-propagation test: route `direct:start` with doTry whose body calls `direct:inner`; `direct:inner` throws `code: "ORDER_NOT_FOUND"`; caller catch clause matches that code → caller catch runs.
2. Fire-and-forget seda test: consumer route on `seda:jobs` with clause `[{code: "ORDER_NOT_FOUND", handled: true, handled_by: "direct:dlq-shim"}]`; producer sends InOnly then sets a body marker → consumer-side clause runs, producer exchange completes normally without the error.
3. Waiting seda test: InOut send to `seda:jobs` whose consumer throws → producer observes the consumer-side failure per the existing waiting-delivery contract.

**Tests:**
- `direct_propagates_business_error_to_caller`: setup as (1) → caller catch clause matched.
- `seda_fire_and_forget_producer_blind_consumer_owns`: setup as (2) → `direct:dlq-shim` received the failed exchange; producer marker body intact; producer outcome success.
- `seda_in_out_delivers_consumer_failure`: setup as (3) → producer call surfaces the consumer-side failure (existing contract shape).

**Test command:** `cargo test -p camel-core --test business_error_propagation_tests`
**Expected before implementation:** tests compile but fail on the seda waiting-mode expectation only if that contract already exists — write them first and record actual RED/GREEN status per test; `direct` propagation is expected GREEN already (existing contract), serving as a regression pin.

**Acceptance:**
- All three tests pass against the existing contracts (no production change expected in this task; any failure is a bug found, not a feature to build here).
- `cargo fmt --check` clean; `cargo clippy -p camel-core -- -D warnings` exit 0.

- [x] 1.7

## Phase 2: HTTP mapping and rest inheritance

Map business errors to HTTP through one trusted table on a separate route-level field, with the never-2xx closure, the rc-7lkp2 Ok-arm fix, and `rest:` inheritance. Matches `design.md ## Phases` Phase 2; depends on Phase 1.

### Task 2.1: HttpErrorMapping type, route-level field, finalizer mapping + rc-7lkp2 Ok-arm

**Files:**
- `crates/camel-api/src/http_error_mapping.rs` (new)
- `crates/camel-api/src/lib.rs` (modified — module registration)
- `crates/components/camel-http/src/lib.rs` (modified)
- `crates/components/camel-http/src/lib_tests.rs` (modified)

**Steps:**
1. Add `HttpErrorMapping { codes: BTreeMap<String, u16>, kinds: BTreeMap<String, u16> }` in a new camel-api module `http_error_mapping` (sibling of the error-handler module — the mapping is orthogonal to it, review m7) with a validation function: blank keys and values outside 400-599 are rejected (typed error). The type is route-level: it is NOT added to `ErrorHandlerConfig` (owner decision Q1); carriage happens through the route translation in task 2.2.
2. Extend `pipeline_error_to_reply` (`camel-http/src/lib.rs:3683`) with an `Option<&HttpErrorMapping>` parameter; update its three call sites (`:2283`, `:2297`, `:2415`). Resolution precedence: explicit code map, explicit kind map, built-in default kind map (kebab-case: `invalid-argument` 400, `conflict` 409, `not-found` 404, `forbidden` 403, `failed` 500), 500. An out-of-domain (non-400-599) entry is treated as absent at reply time and normal precedence resumes. Body: exactly `{"error": <code>, "kind": <kind>}`; never `message`/`data`; never 2xx. Non-business errors keep their existing arms; `None` mapping still applies the default kind map and 500 closure. The finalizer owns the propagate log for seam routes: info when the effective mapped status is below 500, warn at 500-or-above including the built-in 500 closure (owner decision Q7; the error handler logs debug on seam routes — review I1, no double record).
3. Close the rc-7lkp2 hole in the reply arm (C1): the `Ok` reply path (`camel-http/src/lib.rs:2329-2339` area) checks whether the returned exchange still carries an error state; if it does, the exchange is mapped like any other pipeline error — the route's error mapping for a business error, the EXISTING variant mappings for non-business errors (ValidationError 400, ConsumerStopping 503, and so on) — and never answers 2xx. `rc-7lkp2` stays the tracking issue; this fix rides the mapping work.

**Tests:** (unit-level finalizer tests in `crates/components/camel-http/src/lib_tests.rs`, the existing in-file test module; package name is `camel-component-http`, directory is `camel-http`)
- `code_map_wins_over_kind_map`: mapping `{codes: {"ORDER_NOT_FOUND": 404}, kinds: {"not-found": 410}}`; finalizer receives `Business { code: "ORDER_NOT_FOUND", kind: "not-found" }` → status 404, body `{"error":"ORDER_NOT_FOUND","kind":"not-found"}`.
- `kind_map_applies_without_code_match`: mapping `{kinds: {"conflict": 409}}`; error kind `conflict` → 409.
- `builtin_default_map_without_mapping`: no mapping; kinds `invalid-argument`, `conflict`, then `forbidden` → 400, 409, 403 (review m3: `conflict` pins the canonical fallback-arm scenario).
- `unmapped_kind_falls_back_500`: no mapping; kind `custom-kind` → 500, body still only `error`/`kind` (review m6).
- `propagate_below_500_logs_info`: mapping maps the kind to 404; unmatched business error propagates to the finalizer → propagate record at info (review I1: transport-owned).
- `propagate_at_or_above_500_logs_warn`: mapping maps the kind to 502; and a second route with empty tables (built-in 500 closure) → both propagate records at warn (owner decision Q7).
- `load_rejects_out_of_domain_statuses`: validation function receives 101, 200, 304, 700 → each rejected.
- `load_rejects_blank_keys`: blank code key and blank kind key → rejected.
- `programmatic_invalid_entry_treated_absent`: programmatically built mapping with `not-found → 204` → reply is 404 (built-in map); with a valid sibling kind entry → that entry's status; with an invalid unknown-kind entry → 500.
- `message_and_data_never_in_body`: error with populated `message` and `data` under any mapping → body contains only `error` and `kind`.
- `ok_exchange_with_error_never_2xx`: reply arm receives an `Ok` exchange whose error is present (the rc-7lkp2 boundary-propagate shape) → reply maps the carried error through the EXISTING variant mappings for non-business errors and the route's error mapping for a business error — never 2xx (review round 4: carried mappings preserved).
- `ok_with_validation_error_maps_400`: `Ok` exchange carrying `ValidationError("invalid")` → reply 400 (existing variant mapping reused).
- `ok_with_consumer_stopping_maps_503`: `Ok` exchange carrying `ConsumerStopping` → reply 503 (existing shutdown mapping reused).

**Test command:** `cargo test -p camel-component-http --lib` and `cargo test -p camel-api --lib`
**Expected before implementation:** tests fail to compile (type/parameter missing) — RED.

**Acceptance:**
- All task tests pass; existing `pipeline_error_to_reply` tests (TypeConversionFailed 500 per ADR-0096, ConsumerStopping 503, validation 400) stay green.
- `cargo clippy -p camel-api -- -D warnings` and `cargo clippy -p camel-component-http -- -D warnings` exit 0.
- `cargo fmt --check` clean.

- [x] 2.1

### Task 2.2: route-level mapping field, rest lowering, consumer seam

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/model.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)
- `crates/camel-dsl/src/rest.rs` (modified)
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified)
- `crates/components/camel-component-api/src/consumer.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_ext.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_runtime_state.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified)
- `crates/camel-test/tests/business_error_http_test.rs` (new)
- `crates/camel-dsl/tests/rest_error_mapping_tests.rs` (new)

**Steps:**
1. Add `error_mapping: Option<HttpErrorMappingTable>` to the rest block definition parsed from YAML (`route_ast.rs` rest structure, `deny_unknown_fields`), where the table type mirrors `{codes, kinds}` with serde maps; validate with the same load-time rules as task 2.1 (blank keys, 400-599 only).
2. Add `http_error_mapping: Option<HttpErrorMapping>` as a SEPARATE route-level field on `RouteDslRoute` (`route_ast.rs`), `DeclarativeRoute` (`model.rs`), and the compiled `RouteDefinition` (`camel-core/src/lifecycle/application/route_definition.rs:708`, with a `with_http_error_mapping` builder the controller reads — review I3), carried through the shared translation into the compiled route. The `RouteDslRoute` field is `#[serde(skip_deserializing)]`, schema-skipped, and `#[ts(skip)]` (the internal-field pattern of `RouteDslStep::ResponseMarshal`, review m4): plain-route YAML authoring is deferred to the phase-3 `errors:` catalog (owner decision Q5), and `deny_unknown_fields` rejects the key on route documents. `error_handler` is NOT touched anywhere: rest lowering keeps `error_handler: None` exactly as today (`rest.rs:486`) and ALWAYS sets `http_error_mapping: Some(_)` — with empty tables when the block declares none — marking lowered routes as seam routes (review C1).
3. Seam-conditional business dead-letter bypass (review C1): plumb the seam flag from the route compiler to the handler — exclude seam business errors from the synthetic Handled catch-all policy (`route_compiler_ext.rs:77-86`) AND from the default dead-letter producer, while preserving explicit handled clauses; suppressing the synthetic DLC producer must not clear the error (`error_handler.rs:349-353`), so the exchange reaches the reply arm carrying the error and the rc-7lkp2 Ok-arm guard applies. The bypass also covers a matched clause without `handled_by` (`policy_producer.or(dlc)`, `error_handler.rs:179`). A business error never reaches the no-error-handler `error!` terminal site (`error_handler.rs:710`). Non-seam routes keep today's dispatch unchanged.
4. Canonical-path carriage (review I2): `compile_declarative_route_to_canonical` (`compile.rs:324-350`, which guards `error_handler` and `security_policy`) SHALL handle `http_error_mapping` — reject it when `!allow_loss` and record it in `CanonicalLossReport` otherwise — so the mapping is never dropped silently on the canonical/hot-reload path.
5. Consumer seam: registration happens in the route controller (`route_controller_trait.rs:375` start and `:983` resume paths). Persist the mapping snapshot in the route's runtime state (`route_runtime_state.rs`) and install it on `ConsumerContext` (`camel-component-api/src/consumer.rs:190`) on BOTH start and resume — wire the capture in `route_controller.rs` before the handler configuration is consumed — so suspend/resume preserves the mapping. The camel-http consumer (`camel-http/src/lib.rs`, consumer `start(ctx)` at `:2035`) captures it per registration and passes it to `pipeline_error_to_reply` per invocation. No global or ambient lookup.

**Tests:** (DSL-side in `crates/camel-dsl/tests/rest_error_mapping_tests.rs`; consumer-side observable tests in `crates/camel-test/tests/business_error_http_test.rs`, staged listener from `camel-test/tests/support/mod.rs` — component-crate tests cannot reach that support module, so the snapshot proof lives in camel-test)
- `rest_block_parses_error_mapping`: rest block with `error_mapping: {kinds: {"not-found": 410}}` loads → table present on the parsed rest definition.
- `rest_lowering_sets_route_field_keeps_error_handler_none`: block with mapping → every lowered operation route carries `http_error_mapping` set AND `error_handler: None` (Q1: the mapping never touches the error handler).
- `rest_without_mapping_sets_empty_tables`: block without `error_mapping` → lowered routes carry `http_error_mapping: Some(_)` with empty tables and `error_handler: None` (review C1: seam marking).
- `canonical_path_never_drops_mapping_silently`: a route with a mapping compiled through `compile_declarative_route_to_canonical` → the mapping is recorded in `CanonicalLossReport` (or compilation is rejected when `!allow_loss`); it is never silently absent (review I2).
- `seam_route_business_error_skips_dlc_and_synthetic_policy` (in `crates/camel-test/tests/business_error_http_test.rs`): rest route (seam, `Some(empty tables)`) with a global dead-letter channel → unmatched business error is NOT dispatched to the DLC, NOT consumed by the synthetic Handled catch-all (the error is not cleared), and the reply maps through the built-in default kind map, never 2xx (review C1).
- `matched_clause_without_handled_by_fallback_bypassed_on_seam`: seam route, matched clause without `handled_by` (which normally falls back to the DLC via `policy_producer.or(dlc)`) → the clause's disposition applies without dead-letter fallback, and the reply maps (review C1).
- `seam_route_propagate_logs_debug_at_error_handler`: seam route; unmatched business error propagates → error handler logged debug only (the info/warn propagate record is the finalizer's, tested in 2.1; no double record — review I1).
- `rest_mapping_rejects_out_of_domain_status`: rest blocks with `error_mapping` values 200, 101, 304, and 700 → each load fails with a typed error.
- `rest_mapping_rejects_blank_key`: rest block with a blank code key or blank kind key → load fails naming the key.
- `route_error_handler_block_and_route_field_reject_mapping`: a route document's `error_handler:` block with `http_error_mapping`, and a route document carrying a top-level `http_error_mapping` key → each load fails as unknown field (the serde-skip fence; plain-route YAML deferred to phase 3).
- `mapping_snapshot_survives_resume` and `per_registration_isolation` (in `crates/camel-test/tests/business_error_http_test.rs`, OBSERVABLE proof via staged listener — no private consumer-state inspection): two routes with DIFFERENT mappings (e.g. `not-found → 410` vs `not-found → 404`) served through their consumers reply with their own statuses (isolation); after a `suspend_route → resume_route` cycle (stopped routes cannot resume) the replies keep the route's own mapping (resume preservation).

**Test command:** `cargo test -p camel-dsl --test rest_error_mapping_tests` and `cargo test -p camel-test --test business_error_http_test`
**Expected before implementation:** new tests fail (unknown field / field never set) — RED.

**Acceptance:**
- All task tests pass; `cargo xtask schema --check` exits 0 (rest block schema updated; route field schema-skipped).
- `cargo clippy -p camel-dsl -- -D warnings`, `cargo clippy -p camel-component-api -- -D warnings`, `cargo clippy -p camel-core -- -D warnings`, and `cargo clippy -p camel-component-http -- -D warnings` exit 0.
- `cargo fmt --check` clean.

- [x] 2.2

### Task 2.3: end-to-end HTTP mapping, DLC coexistence, rc-7lkp2 coverage

**Files:**
- `crates/camel-test/tests/business_error_http_test.rs` (modified — extended from task 2.2's snapshot tests)

**Steps:**
1. E2E mapped status via rest inheritance: rest block with `error_mapping: {kinds: {"not-found": 410}}` (deliberate override — the built-in default kind map already maps `not-found` to 404, so a 404 expectation cannot discriminate the block table from the built-in map) whose operation pipeline throws `code: "ORDER_NOT_FOUND", kind: "not-found"` → HTTP reply 410, body exactly `{"error":"ORDER_NOT_FOUND","kind":"not-found"}`.
2. E2E rest + global DLC + mapping (Q1 coexistence): operation route with a global `error_handler` dead-letter channel configured via the DSL programmatic path, plus the rest block mapping — a business error is NOT dispatched to the DLC and the reply carries the mapped status (never 2xx); a system error (`Io`) on the same route IS forwarded to the DLC.
3. E2E catch-then-map: doTry around the throw, catch clause by code rewrites the body and the exchange continues → reply 2xx from the normal path (a mapped business error never happens because the route handled it).
4. E2E rest inheritance + built-in map: same rest block, second operation raising kind `conflict` → 409 from the built-in default kind map beneath the block table.
5. E2E unmapped closure: rest block without mapping (empty tables), operation raises kind `custom-kind` → 500, never 2xx; body never contains `message` or `data`.
6. E2E rc-7lkp2 path coverage (C1): a handler-configured route whose boundary-propagate path returns `Ok(exchange)` with the error still present → the reply maps the carried error and is never 2xx.
Use the real Camel consumer through the staged listener `support::stage_http_listener` (`camel-test/tests/support/mod.rs`) — `HttpTestServer` serves fake responses and cannot exercise the mapping path; keeping the tests in camel-test respects `lint-no-raw-http-test-server` and component-dep limits.

**Tests:**
- `e2e_rest_mapped_status_via_block_table`: setup as (1) → status 410 (block-table override beats the built-in 404) and exact body asserted.
- `e2e_rest_global_dlc_and_mapping_coexist`: setup as (2) → business error replies with the mapped status, never 2xx; DLC receives the system error but not the business error.
- `e2e_caught_business_error_is_normal_response`: setup as (3) → 2xx with the catch body, no business mapping involved.
- `e2e_rest_inheritance_two_operations_one_table`: setups (1) and (4) → 410 and 409 from one table.
- `e2e_unmapped_rest_business_error_is_500`: setup as (5) → 500, body only `error`/`kind`.
- `e2e_ok_with_error_boundary_propagate_never_2xx`: setup as (6) → non-2xx mapped reply (rc-7lkp2 path covered).

**Test command:** `cargo test -p camel-test --test business_error_http_test`
**Expected before implementation:** e2e tests fail (mapping not wired end-to-end until 2.1+2.2 land; this task pins the whole chain) — RED until prior tasks complete, GREEN after.

**Acceptance:**
- All six e2e tests pass.
- The rc-7lkp2 fix lands with this epic: the implementation commit references bd `rc-7lkp2` and the issue is closed with the reference once the Ok-arm fix ships (review m5).
- `cargo test -p camel-component-http --lib` (finalizer + Ok-arm tests from 2.1) stays green.
- `cargo clippy -p camel-component-http -- -D warnings` and `cargo clippy -p camel-test -- -D warnings` exit 0; `cargo fmt --check` clean.

- [x] 2.3
