# Tasks: correlationfix

## camel-api

### Task 1.1: Add `AggregatorConfigBuilder::correlate_by_expr` setter

**Files:**
- `crates/camel-api/src/aggregator.rs` (modified)

**Steps:**
1. Add method `pub fn correlate_by_expr(mut self, expr: impl Into<String>, language: impl Into<String>) -> Self` to `impl AggregatorConfigBuilder` (after the existing `correlate_by` override at ~line 328). It sets `self.correlation = CorrelationStrategy::Expression { expr: expr.into(), language: language.into() }` and returns `self`. It does NOT touch `header_name` (mirror of `correlate_by` override semantics). Doc comment: one line stating override semantics and that runtime correlation reads `config.correlation`.
2. Add a unit test in the aggregator.rs test module (locate the existing `#[cfg(test)] mod tests` in this file; if none exists, create `#[cfg(test)] mod correlation_tests` at file end).

**Tests:** (executable spec)
- `correlate_by_expr_overrides_strategy_and_keeps_header_name`: setup `AggregatorConfig::correlate_by("orderId")` → action `.correlate_by_expr("${header.orderId}", "simple").complete_when_size(2).build()` → assert `matches!(config.correlation, CorrelationStrategy::Expression { ref expr, ref language } if expr == "${header.orderId}" && language == "simple")` AND `config.header_name == "orderId"`.
  - command: `RUSTC_WRAPPER= cargo test -p camel-api --lib correlate_by_expr`
  - expected: fails before step 1, passes after.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-api --lib correlate_by_expr` exits 0.
- `RUSTC_WRAPPER= cargo clippy -p camel-api -- -D warnings` exits 0.
- `RUSTC_WRAPPER= cargo fmt --check` exits 0.

- [x] 1.1

### Task 1.2: `validate_contract` aggregate rule becomes correlation-source one-of

**Files:**
- `crates/camel-api/src/runtime.rs` (modified)

**Steps:**
1. In `validate_steps` / the `CanonicalStepSpec::Aggregate(config)` arm (~lines 466-481), replace the unconditional empty-header rejection with the one-of rule: build the pair `(header_present, key_present)` = (`!config.header.trim().is_empty()`, `config.correlation_key.as_deref().is_some_and(|k| !k.trim().is_empty())`).
2. Error cases (both `CamelError::RouteError` with `"canonical contract violation: "` prefix, matching sibling messages):
   - if `!key_present && config.correlation_key.is_some()` → message `"canonical contract violation: aggregate.correlation_key cannot be empty"` (empty expression beside a non-empty header is not a source),
   - else if `!header_present && !key_present` → message `"canonical contract violation: aggregate requires a correlation source: header or correlation_key"`.
3. Empty header WITH non-empty correlation_key passes (no error). The existing `completion_size == 0` check stays unchanged.
4. Update the existing test `canonical_contract_rejects_invalid_aggregate_and_circuit_breaker` (~runtime.rs:836-853): its whitespace-header aggregate case now triggers the correlation-source error — change the expected substring from `"aggregate.header cannot be empty"` to `"correlation source"`.
5. Add unit tests co-located with the existing runtime.rs tests. `CanonicalAggregateSpec` has NO `Default` impl — construct every field explicitly, modeled on the existing test at runtime.rs:838.

**Tests:** (executable spec — construct `CanonicalAggregateSpec` with ALL fields explicit, modeled on runtime.rs:838; no `Default` impl exists)
- `contract_accepts_expression_only_aggregate`: setup `CanonicalAggregateSpec` with `header: ""`, `correlation_key: Some("${header.orderId}")`, `strategy: CollectAll`, remaining fields `None` (explicitly) → action `validate_contract()` (or `validate_steps` — use whatever the existing tests at runtime.rs:836 call) → assert `Ok(())`.
- `contract_rejects_aggregate_missing_both_sources`: setup header `""`, correlation_key `None` → action validate → assert `Err` whose message contains `"correlation source"`.
- `contract_rejects_empty_correlation_key`: setup header `"region"`, correlation_key `Some("")` → action validate → assert `Err` whose message contains `"correlation_key cannot be empty"`.
  - command: `RUSTC_WRAPPER= cargo test -p camel-api --lib contract_`
  - expected: new tests fail before step 1-2, pass after.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-api --lib` exits 0 (FULL suite — catches the updated legacy test plus the 3 new ones).
- `RUSTC_WRAPPER= cargo clippy -p camel-api -- -D warnings` exits 0.
- `RUSTC_WRAPPER= cargo fmt --check` exits 0.

- [x] 1.2

## camel-dsl

### Task 2.1: Make `aggregate.header` optional at YAML parse + regenerate schemas

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified — new parse test)
- `schemas/dsl/route-schema.json` (modified, regenerated)
- `crates/camel-lint/schema/route-schema.json` (modified, regenerated)
- `schemas/ts/AggregateData.ts` (modified, regenerated — step 1 changes `AggregateData`; append any other exact TS paths `git status` reports after regen)

**Steps:**
1. In `AggregateData` (~line 956), change `pub header: String` to carry `#[serde(default)]` directly above it (field type stays `String`, default `String::new()`). Do not change any other field.
2. Run `RUSTC_WRAPPER= cargo xtask schema` from the worktree root to regenerate on-disk schemas (`schemas/dsl/`, `schemas/ts/`, canonical schema).
3. Inspect the schema diff: the aggregate verb's `header` property moves out of the `required` array; no unrelated drift (if unrelated drift appears, STOP and report rather than committing it).

**Tests:** (executable spec)
- `aggregate_yaml_parses_without_header_key`: setup a YAML route string with an aggregate step carrying only `correlation_key: "${header.orderId}"` and `completion_size: 2` → action parse via the existing YAML route parse entry used by neighboring tests in `crates/camel-dsl/src/yaml.rs` tests → assert the parsed `AggregateStepDef.header == ""` and `correlation_key == Some("${header.orderId}")`.
  - command: `RUSTC_WRAPPER= cargo test -p camel-dsl --lib aggregate_yaml_parses_without_header_key`
  - expected: fails before step 1, passes after.
- Schema check (gate, not a cargo test): `RUSTC_WRAPPER= cargo xtask schema --check` exits 0 after step 2.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-dsl --lib aggregate_yaml_parses_without_header_key` exits 0.
- `RUSTC_WRAPPER= cargo xtask schema --check` exits 0.
- Generated aggregate-verb schema no longer lists `header` in `required`.

- [x] 2.1

### Task 2.2: Rewrite declarative validation to requires-one-of

**Files:**
- `crates/camel-dsl/src/compile.rs` (modified)

**Steps:**
1. In `validate_step`, `DeclarativeStep::Aggregate(def)` arm (~lines 2108-2114), replace the `correlation_key.is_none()` rejection with:
   - if `def.correlation_key.as_deref().is_some_and(|k| !k.trim().is_empty())` → Ok (expression source present),
   - else if `def.correlation_key.is_some()` → `Err(CamelError::Config("aggregate.correlation_key cannot be empty".to_string()))`,
   - else if `!def.header.trim().is_empty()` → Ok (header source present),
   - else → `Err(CamelError::Config("aggregate requires a correlation source: header or correlation_key".to_string()))`.
2. Update the existing test `test_aggregate_requires_correlation_key` (~line 4351): it must now construct a def with `header: String::new()` AND `correlation_key: None` to trigger the error, and assert the new message contains `"correlation source"`. Rename it `aggregate_missing_both_sources_is_rejected`. Note: `AggregateStepDef` has NO `Default` — construct all 11 fields explicitly, modeled on the existing test at compile.rs:4351.

**Tests:** (executable spec)
- `aggregate_missing_both_sources_is_rejected` (renamed existing): setup `AggregateStepDef` with `header: ""`, `correlation_key: None`, all other fields empty/`None` (full-field construction) → action `validate_step` → assert `Err(CamelError::Config(_))` containing `"correlation source"`.
- `aggregate_empty_correlation_key_is_rejected`: setup `header: "region"`, `correlation_key: Some("")` → action `validate_step` → assert `Err` containing `"correlation_key cannot be empty"`.
- `aggregate_header_only_passes_validation`: setup `header: "orderId"`, `correlation_key: None` → action `validate_step` → assert `Ok(())`.
  - command: `RUSTC_WRAPPER= cargo test -p camel-dsl --lib aggregate_`
  - expected: new cases fail before step 1, pass after.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-dsl --lib aggregate_` exits 0.
- `RUSTC_WRAPPER= cargo clippy -p camel-dsl -- -D warnings` exits 0.
- `RUSTC_WRAPPER= cargo fmt --check` exits 0.

- [x] 2.2

### Task 2.3: Shared correlation helper + wire builder lowering path

**Files:**
- `crates/camel-dsl/src/compile.rs` (modified)

**Steps:**
1. Add free function in compile.rs (near `compile_canonical_aggregate`):
   `fn aggregate_correlation(header: &str, correlation_key: Option<&str>) -> CorrelationStrategy` — returns `Expression { expr: correlation_key.to_string(), language: "simple".to_string() }` when `correlation_key` is `Some`, else `HeaderName(header.to_string())`. Import `CorrelationStrategy` from `camel_api::aggregator` (check the existing import block first and extend it).
2. In `compile_canonical_aggregate` (~lines 738-744), replace the inline `if let Some(expr) = config.correlation_key { ... }` override block with `agg_config.correlation = aggregate_correlation(&config.header, config.correlation_key.as_deref());` (keeping the move-order constraints noted in the comment above the completion_predicate block — the helper borrows, so call it after the predicate block).
3. In `compile_aggregate_step` (~lines 1828-1878): delete the stale NOTE comment at ~1839-1841 ("intentionally not wired"); after `builder.build()?` assign `agg_config.correlation = aggregate_correlation(&def.header, def.correlation_key.as_deref());` (bind `let mut agg_config = builder.build()?;` first).
4. Update the `completion_predicate` rejection message (~line 1833): drop the clause `"or correlation keys"` — new message: `"aggregate.completion_predicate requires the canonical route path (the builder path does not lower expression predicates)"`.
5. Add tests in the compile.rs test module near the existing aggregate tests (~4300+).

**Tests:** (executable spec — `CorrelationStrategy` has NO `PartialEq`: always assert with `matches!` + binding guards, never `==`; `AggregateStepDef` has NO `Default`: construct all fields explicitly, modeled on compile.rs:4351)
- `builder_path_lowers_correlation_key_expression`: setup `AggregateStepDef` with `header: "region"`, `correlation_key: Some("${header.orderId}")`, `completion_size: Some(2)` (all other fields their empty/None values) → action `compile_aggregate_step(def)` → assert `matches!(config.correlation, CorrelationStrategy::Expression { ref expr, ref language } if expr == "${header.orderId}" && language == "simple")`.
- `builder_path_header_only_keeps_header_strategy`: setup header `"orderId"`, correlation_key `None` → action `compile_aggregate_step` → assert `matches!(config.correlation, CorrelationStrategy::HeaderName(ref h) if h == "orderId")`.
- `builder_path_expression_only_empty_header_lowers_expression`: setup header `""`, correlation_key `Some("${body.id}")` → action `compile_aggregate_step` → assert `matches!(config.correlation, CorrelationStrategy::Expression { ref expr, ref language } if expr == "${body.id}" && language == "simple")`.
- `builder_path_predicate_error_no_longer_mentions_correlation_keys`: setup def with `completion_predicate: Some(...)` (any valid `LanguageExpressionDef`) → action `compile_aggregate_step` → assert `Err` message contains `"completion_predicate"` and does NOT contain `"correlation keys"`.
  - command: `RUSTC_WRAPPER= cargo test -p camel-dsl --lib builder_path_`
  - expected: first three fail before steps 1+3, pass after.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-dsl --lib builder_path_` exits 0 (4 tests).
- `RUSTC_WRAPPER= cargo test -p camel-dsl --lib canonical_aggregate` exits 0 (existing canonical tests unbroken by step 2).
- `RUSTC_WRAPPER= cargo clippy -p camel-dsl -- -D warnings` exits 0.

- [x] 2.3

### Task 2.4: Canonical-path parity tests

**Files:**
- `crates/camel-dsl/src/compile.rs` (modified)

**Steps:**
1. Add tests next to the canonical aggregate tests (~3419-3549 region) covering the same three defs as Task 2.3 through the canonical path: `compile_aggregate_step_to_canonical` then `compile_canonical_aggregate`. Construct `AggregateStepDef` with all fields explicit (no `Default`).
2. Add one table-driven parity test iterating the three shapes (header-only / expression-only / both-present), compiling each through BOTH paths, then comparing WITHOUT `==` (no `PartialEq` on `CorrelationStrategy`): destructure both strategies with `matches!`-style guards and assert variant name + bound `expr`/`language`/header strings are equal pairwise.
3. Add the recompile half of the spec's builder-canonicalization round-trip scenario (camel-builder cannot host it — it has no camel-dsl dependency; Task 3.1 asserts the canonicalize half on the builder side): construct the EXACT `CanonicalAggregateSpec` shape `canonicalize_aggregate` emits for an `Expression` config — `header` set to the expression text AND `correlation_key: Some(expr)` — and compile it through `compile_canonical_aggregate`.

**Tests:** (executable spec)
- `canonical_path_lowers_correlation_key_expression`: setup same def as `builder_path_lowers_correlation_key_expression` → action `compile_aggregate_step_to_canonical(def)` then `compile_canonical_aggregate(spec)` → assert `matches!(.., CorrelationStrategy::Expression { ref expr, ref language } if expr == "${header.orderId}" && language == "simple")`.
- `canonical_path_expression_only_empty_header_lowers_expression`: header `""`, correlation_key `Some("${body.id}")` → action same chain → assert `Expression { expr: "${body.id}", language: "simple" }` via `matches!` guard.
- `canonical_and_builder_paths_agree_on_correlation`: setup table `[("orderId", None), ("", Some("${body.id}")), ("region", Some("${header.orderId}"))]` → action compile each def through both paths → assert per-row: both strategies are the same variant and all bound string fields (header name, or expr+language) are equal.
- `canonical_recompile_of_canonicalized_expression_config_yields_expression`: setup `CanonicalAggregateSpec` with `header: "${header.orderId}"` (duplicated expression text, exactly as `canonicalize_aggregate` emits it) and `correlation_key: Some("${header.orderId}")` → action `compile_canonical_aggregate` → assert `Expression { expr: "${header.orderId}", language: "simple" }`. Together with Task 3.1's `canonicalize_aggregate_expression_maps_correlation_key` this completes the round-trip scenario.
  - command: `RUSTC_WRAPPER= cargo test -p camel-dsl --lib canonical`
  - expected: fail before Task 2.3 lands, pass after 2.3 + this task.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-dsl --lib canonical` exits 0 (4 new + all pre-existing canonical tests).
- `RUSTC_WRAPPER= cargo clippy -p camel-dsl -- -D warnings` exits 0.

- [x] 2.4

### Task 2.5: Route-level YAML tests through both entry points

**Files:**
- `crates/camel-dsl/src/compile.rs` (modified) — test module only

**Steps:**
1. Add route-level tests that start from YAML source strings, parse them with `camel_dsl::yaml::parse_yaml_to_declarative` (crates/camel-dsl/src/yaml.rs:106 — returns the `DeclarativeRoute` both compile entries consume), then run BOTH `compile_declarative_route` and `compile_declarative_route_to_canonical`, and assert the aggregate correlation on the compiled route (builder path) and the canonical spec (canonical path).

**Tests:** (executable spec)
- `yaml_expression_only_aggregate_compiles_on_both_paths`: setup YAML route `from: timer:x` with one step `- aggregate: { correlation_key: "${header.orderId}", completion_size: 2 }` (header key absent) → action `parse_yaml_to_declarative` + both compiles → assert builder path yields `Expression { expr: "${header.orderId}", language: "simple" }` (via `matches!` guard; no `==` — `CorrelationStrategy` has no `PartialEq`) AND canonical spec preserves `correlation_key == Some("${header.orderId}")`.
- `yaml_both_present_expression_overrides_header`: setup YAML aggregate with `header: "region"` AND `correlation_key: "${header.orderId}"` → action both compiles → assert builder-path strategy is `Expression` for the expression (not `HeaderName("region")`) and canonical spec carries `correlation_key == Some("${header.orderId}")`.
- `yaml_missing_both_sources_is_rejected_at_route_level`: setup YAML aggregate with `header: ""` and no `correlation_key` → action `compile_declarative_route` → assert `Err` containing `"correlation source"`.
  - command: `RUSTC_WRAPPER= cargo test -p camel-dsl --lib yaml_`
  - expected: first two fail before Tasks 2.1-2.3, pass after; third fails before 2.2, passes after.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-dsl --lib yaml_` exits 0 (plus all pre-existing yaml_ tests).
- `RUSTC_WRAPPER= cargo clippy -p camel-dsl -- -D warnings` exits 0.

- [x] 2.5

## camel-builder

### Task 3.1: Canonicalization round-trip tests for expression correlation

**Files:**
- `crates/camel-builder/src/lib.rs` (modified) — test module only

**Steps:**
1. Locate the existing aggregate canonicalization tests (~3911-4019). Add two tests asserting `canonicalize_aggregate` mapping behavior (function at ~1194-1259): `Expression` → `correlation_key: Some(expr)` with `header == expr`; `HeaderName` → `correlation_key: None` with header preserved. These are the canonicalize HALF of the spec's round-trip scenario; the recompile half is `canonical_recompile_of_canonicalized_expression_config_yields_expression` in Task 2.4 (camel-builder has no camel-dsl dependency, so the halves live in their own crates and are linked here).

**Tests:** (executable spec)
- `canonicalize_aggregate_expression_maps_correlation_key`: setup `AggregatorConfig::correlate_by("seed").correlate_by_expr("${header.orderId}", "simple").complete_when_size(2).build()` → action `canonicalize_aggregate(config)` → assert `spec.correlation_key == Some("${header.orderId}")` AND `spec.header == "${header.orderId}"`.
- `canonicalize_aggregate_header_only_maps_no_correlation_key`: setup `AggregatorConfig::correlate_by("orderId").complete_when_size(2).build()` → action `canonicalize_aggregate` → assert `spec.correlation_key == None` AND `spec.header == "orderId"`.
  - command: `RUSTC_WRAPPER= cargo test -p camel-builder --lib canonicalize_aggregate_`
  - expected: `canonicalize_aggregate_expression_maps_correlation_key` fails before Task 1.1 (no setter), passes after 1.1; the header-only test passes on current code and pins it.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-builder --lib canonicalize_aggregate_` exits 0.
- `RUSTC_WRAPPER= cargo clippy -p camel-builder -- -D warnings` exits 0.

- [x] 3.1

## docs

### Task 4.1: Align aggregate correlation docs to the unified contract

**Files:**
- `docs/src/yaml-dsl/step-verbs.md` (modified)
- `docs/src/eip/aggregator.md` (modified)

**Steps:**
1. `step-verbs.md` aggregate table (~lines 283-299): change the `header` row to Required `no`, Default `""`, description "Header used as the correlation key (header-based source; used when `correlation_key` is absent)". Change the `correlation_key` row description to "Expression correlation source (simple language); overrides `header` when both are present". Add one sentence below the table: at least one non-empty source is required; when both are present the expression wins; an empty `correlation_key` is rejected.
2. `step-verbs.md`: update the aggregate YAML example block (if present near the table) so it stays valid under the new contract; prefer adding `correlation_key: "${header.orderId}"` alongside an existing header example only if it clarifies — do not remove the header-only example.
3. `aggregator.md` (~line 29): extend the paragraph to document both correlation sources — header-based via `correlate_by`, expression-based via `correlate_by_expr` (simple language), one required, expression overrides header when both are set.

**Tests:** (non-Rust — verifiable checks)
- `step-verbs-header-row-optional`: `rg -n '\| `header` \| string \|' docs/src/yaml-dsl/step-verbs.md` shows Required `no` for the aggregate section row.
- `aggregator-md-mentions-expression`: `rg -c 'correlate_by_expr' docs/src/eip/aggregator.md` returns ≥ 1.
  - command: `rg -n "correlate_by_expr" docs/src/eip/aggregator.md && rg -n "correlation_key" docs/src/yaml-dsl/step-verbs.md`
  - expected: both present after edit.

**Acceptance:**
- Both rg checks return matches.
- Docs table no longer claims `header` is required for `aggregate`.
- No prose contradicts override-wins precedence.

- [x] 4.1
