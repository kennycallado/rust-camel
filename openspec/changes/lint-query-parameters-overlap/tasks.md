# Tasks: lint-query-parameters-overlap

## camel-lint (route view + document walk)

### Task 1: Origin-tagged LintOption (OptionOrigin enum)

**Files:**
- `crates/camel-lint/src/route_view.rs` (modified)
- `crates/camel-lint/src/document.rs` (modified)

**Steps:**
1. In `route_view.rs`, add above `LintOption`:
   `#[derive(Clone, Copy, Debug, PartialEq, Eq)] #[non_exhaustive] pub enum OptionOrigin { Query, StepParameters, ConfigParameters }`
   with a doc comment mapping each variant to its lowering vocabulary (Query = raw URI query string; StepParameters = `parameters:` map sibling of a URI-bearing key, incl. route-level `from`; ConfigParameters = `parameters:` map inside an object-form URI key, cf. `combine_params(config, step)` in `camel-dsl/src/yaml.rs`).
2. Add `pub origin: OptionOrigin` to `LintOption`.
3. In `LintOption::parse_from_query`, stamp `origin: OptionOrigin::Query` on both pushed literals (`route_view.rs` ~lines 85 and 93).
4. In `document.rs`, change `collect_parameters(value, path, doc)` to `collect_parameters(value, path, doc, origin: OptionOrigin)` and stamp it on the pushed `LintOption` (~line 461).
5. In `document.rs::walk`, add a `local_origin: OptionOrigin` parameter: the root call in `Document::parse` (~line 56), the CONTAINER_KEYS recursion (~line 414), and the sequence-item recursion (~line 431) pass `OptionOrigin::StepParameters`; the object-form URI-key recursion (~line 407) passes `OptionOrigin::ConfigParameters`. The local collection (~line 361) calls `collect_parameters(pv, path, doc, local_origin)`. Inherited entries keep their tags through the `inherited ++ local` chain (no rewrites).
6. Update every other in-crate `LintOption` construction site to compile (find with `rg -n 'LintOption \{' crates/camel-lint/src`): the `lint_option` test helper in `rules/ruriknown.rs` (~line 353) gains an `origin` field (default the helper parameter list with `OptionOrigin::Query` and add an `origin`-taking variant only if a test needs another value); existing test literals in `route_view.rs`/`document.rs` tests stamp the origin their source implies (query-parsed → `Query`, `collect_parameters`-produced → the origin the walk passes at that site).
7. Export `OptionOrigin` from `lib.rs` alongside `LintOption` (same re-export style).

**Tests:** (in `document.rs` `mod tests`, following the existing `parameters_entries_become_options` pattern — `Document::parse` then inspect `route_view.endpoints()`)
- `query_options_carry_query_origin`: setup = source with `- to: timer:foo?period=1s`; action = parse and take the endpoint's `period` option; assert = `origin == OptionOrigin::Query`.
- `step_parameters_carry_step_origin`: setup = source with `- to: kafka:orders` + sibling `parameters: {brokers: my-host:9092}`; action = take the `brokers` option; assert = `origin == OptionOrigin::StepParameters`.
- `nested_object_form_distinguishes_origins`: setup = object-form `enrich: {uri: db:query, parameters: {dataSource: customers}}` + sibling step-level `parameters: {timeoutS: "5000"}`; action = take the nested endpoint's `dataSource` and `timeoutS` options; assert = `dataSource.origin == ConfigParameters` AND `timeoutS.origin == StepParameters`.
- `from_parameters_carry_step_origin`: setup = `from: timer:tick` + route-level `parameters: {period: "2500"}`; action = take the `from` endpoint's `period` option (via `route_view.endpoints()`, which folds `from_parameters` in); assert = `origin == OptionOrigin::StepParameters`.
- command = `cargo test -p camel-lint --lib`; expected = all four NEW tests fail before steps 1–6, pass after; the full existing suite still passes (span assertions unchanged).

**Acceptance:**
- `cargo test -p camel-lint --lib` exits 0.
- `cargo clippy -p camel-lint -- -D warnings` exits 0.
- `rg -n 'origin:' crates/camel-lint/src/route_view.rs` shows the field; no `LintOption` literal compiles without an origin (verified by the passing build).

- [x] 1.1

## camel-lint (diagnostic + rule)

### Task 2: UriKnownSubCode::DuplicateKey and the R-URI-known check

**Prerequisites:** Task 1 (the rule reads `LintOption.origin`).

**Files:**
- `crates/camel-lint/src/diagnostic.rs` (modified)
- `crates/camel-lint/src/rules/ruriknown.rs` (modified)

**Steps:**
1. In `diagnostic.rs`, add variant `DuplicateKey` to `UriKnownSubCode` (a plain `pub enum` — only the in-crate `Display` match needs the new arm; do NOT add `#[non_exhaustive]`).
2. Extend the `Display` impl match arm list with `UriKnownSubCode::DuplicateKey => "duplicate-key"`, and extend the stable-contract doc comment on the `Display` impl so the sub-code list includes `duplicate-key`.
3. In `rules/ruriknown.rs::analyze_endpoint`, insert a duplicate-key pass as the FIRST thing in the function (before the colon/scheme split, so it also fires for URIs the catalog cannot verify): one pass over `ep.options` grouping by raw key (`opt.key.value`, no alias resolution); for each key record which origins occur (`Query`/`StepParameters`/`ConfigParameters`) and the span of the FIRST non-Query occurrence (options order = query options first, then step-level inherited, then config local — so the first non-Query occurrence is the step-level key when both parameter sides are present, matching `combine_params` which names the step key, and the parameter key for query∩parameters, matching `try_from_uri_and_params`). For each key occurring in ≥2 distinct origins, push ONE `Diagnostic { code: DiagnosticCode::RUriKnown(UriKnownSubCode::DuplicateKey), severity: Severity::Error, span: <that first non-Query key span>, message: format!("duplicate option key `{key}`: declared in {sources}") }` where `{sources}` names the distinct origins in the fixed vocabulary `the URI query string` / `step parameters` / `config parameters` joined with ` and `, `fix: None`.
4. Keep the existing per-occurrence validation unchanged (both occurrences still get unknown-option/kind checks).

**Tests:** (rule-level, in `ruriknown.rs` `mod tests` — `Document::parse` + `RUriKnownRule.analyze` + `StubCatalog`; follow the existing `meta_with_options` helper)
- `duplicate_key_display_string`: setup = the enum variant; action = `DiagnosticCode::RUriKnown(UriKnownSubCode::DuplicateKey).to_string()`; assert = equals `"R-URI-known:duplicate-key"`.
- `query_and_step_parameters_overlap_flagged`: setup = source `- to: timer:foo?period=1000` + sibling `parameters: {period: "2500"}`, catalog `timer` with option `period`; action = analyze; assert = exactly one `DuplicateKey` diagnostic, severity Error, span start == byte offset of the `period` key inside the `parameters:` map (second `period` occurrence in the source), zero other DuplicateKey diagnostics.
- `config_and_step_parameters_overlap_flagged`: setup = object-form `enrich: {uri: db:query, parameters: {timeout: "1"}}` + sibling step-level `parameters: {timeout: "2"}`; action = analyze; assert = exactly one `DuplicateKey` diagnostic with span on the step-level `timeout` key (the second `timeout` occurrence in the source).
- `repeated_query_keys_not_flagged`: setup = `- to: timer:foo?period=1&period=2`, catalog `timer`; action = analyze; assert = zero `DuplicateKey` diagnostics.
- `overlap_flagged_for_unregistered_scheme`: setup = `- to: kafka:orders?brokers=h1` + `parameters: {brokers: "h2"}`, catalog WITHOUT `kafka`; action = analyze; assert = one `DuplicateKey` error on the parameters-side `brokers` key AND one informational `UnverifiedScheme` note (both present).
- `route_level_from_overlap_flagged`: setup = `from: timer:tick?period=1s` + route-level `parameters: {period: "2500"}`, catalog `timer`; action = analyze; assert = one `DuplicateKey` diagnostic with span inside the route-level `parameters:` map.
- `all_three_sources_single_diagnostic`: setup = object-form `- to: {uri: timer:foo?period=1s, parameters: {period: "2"}}` + step-level `parameters: {period: "3"}`, catalog `timer`; action = analyze; assert = EXACTLY one `DuplicateKey` diagnostic for key `period` on that endpoint.
- command = `cargo test -p camel-lint --lib`; expected = all seven NEW tests fail before steps 1–3, pass after.

**Acceptance:**
- `cargo test -p camel-lint --lib` exits 0 (new + existing tests).
- `cargo clippy -p camel-lint -- -D warnings` exits 0.
- `cargo fmt --check -p camel-lint` exits 0.

- [x] 2.1

## camel-lint docs + corpus gate

### Task 3: CONTEXT.md origin/diagnostic docs and corpus/production verification

**Prerequisites:** Tasks 1–2.

**Files:**
- `crates/camel-lint/CONTEXT.md` (modified)

**Steps:**
1. In `CONTEXT.md`, extend the rules table row for `R-URI-known` (line ~30) to name the new error: append `duplicate key across query/parameters` to the behavior cell (unknown scheme → Info; unknown option / kind mismatch / missing required / duplicate key → Error).
2. Rewrite the "Endpoint options come from three sources" paragraph (~lines 35–40): it currently claims parameters entries are "indistinguishable from query-string options for rule purposes" and that pre-lowering overlap flagging "is tracked in bd rc-j9v8" — both become false. New text: options carry a source origin (`Query` / `StepParameters` / `ConfigParameters`) distinguishable by rules; a key declared in more than one source is flagged by `R-URI-known:duplicate-key` mirroring the lowering's fail-closed `EndpointUriError::DuplicateKey`. Drop the bd forward reference.
3. Wherever the `UriKnownSubCode` variants or the `R-URI-known:<sub>` stable strings are enumerated (around the `DiagnosticCode` description, line ~85), add `duplicate-key` to the list.
4. Run the corpus gate and the production-catalog gate; confirm zero baseline drift: `cargo test -p camel-cli --test lint_corpus` and `cargo test -p camel-cli --test lint_production_catalog` both pass with `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` UNTOUCHED (verified claim: no corpus fixture has a same-key cross-source collision — `examples/` uses no `parameters:` maps; `parameters-secret.yaml` URIs carry no query strings). If a gate fails because a corpus file has a TRUE collision, update the baseline with the exact new entry and report it — do not weaken the rule.

**Tests:**
- `corpus_baseline_unchanged`: setup = implemented Tasks 1–2; action = `git diff --exit-code crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` after running the corpus test; assert = no diff (exit 0).
- `production_catalog_still_green`: setup = same; action = `cargo test -p camel-cli --test lint_production_catalog`; assert = exit 0.

**Acceptance:**
- Both `cargo test -p camel-cli --test lint_corpus` and `cargo test -p camel-cli --test lint_production_catalog` exit 0.
- `lint-corpus-baseline.ron` byte-identical unless a true collision was found and reported.
- `cargo fmt --check --all` exits 0.

- [x] 3.1
