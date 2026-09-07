# Design: env-int-placeholder-parity

> **Rev 2 addendum (2026-09-07 — bd rc-93wct, e_glm verdict D).** The Context claim "interpolates raw text… so YAML re-infers types" is stale. Superseding mechanics: (1) camel-dsl exposes `pub interpolate_yaml_source` (tree-walk-first, legacy-splice fallback); `load_from_file_with_env` and discovery's `interpolate_for_parse` YAML arm both route through it. (2) camel-lint layers a typing mirror on the SYNC splice mirror: whole-scalar tokens validate as STRING `"d"` (int/bool positions → schema Error); no-default tokens emit Error at any value position (rev-1 leave-literal policy superseded); `$${…}` escapes exempt. D1-D5 otherwise stand.

## Context & ADRs

The design intent is established by CONTEXT-MAP (Config entry): "unified `${env:}` placeholder interpolation (tree walk via camel-dsl `interpolate_env`)" — and by `crates/camel-dsl/src/discovery.rs:340-349`, which interpolates raw **text** before YAML/JSON parsing precisely so YAML re-infers types (integer parametrization is supposed to work). ADR-0069 (integration-tier testing contract) governs tier hermeticity: §13.1 names `global-state` (ambient env mutation) as a flake class — LEAN must not read ambient env. This change touches only parse/lint tooling; the data/control plane boundary is untouched.

## Decisions (papal rulings, e_opus — bd rc-93wct)

- **D1 (Q1)**: LEAN unresolved-no-default = hard `doc_error`, wording mirrors `DiscoveryError::Env` ("environment variable '{var}' not set (required by {path})"). No soft path exists in the runner's `doc_error` model.
- **D2 (Q2)**: Duplicate `interpolate_env_with`'s lookup-only arm into camel-lint + add `regex` dep. Crate purity (route-lint spec: "Engine does not depend on camel-core or camel-dsl") forbids the dep; precedent for deliberate mirrors: `runtime_event_record.rs:10`, `route_ast.rs:142`. Annotate both copies `// SYNC: mirror of camel-dsl::env_interpolation (rc-93wct)`. New leaf crate rejected (publish-surface cost for ~40 lines).
- **D3 (Q3)**: R-SCHEMA emits `Severity::Info` (reuse `DiagnosticCode::RSchema`) per substituted default, anchored on the placeholder value node. Precedent: `UnverifiedScheme` (ruriknown.rs). Info is non-failing; only `Error` sets exit 1.
- **D4 (Q4)**: Land now; do not block on rc-ayke (already-live boot behavior; widening is bounded to commented no-default tokens and degrades to a diagnosable error). Guard test: commented with-default stays harmless.
- **D5 (Q5)**: Zero LSP code. LSP inherits via `engine.lint` (camel-lsp lib.rs); `complete_at`/`hover_at` read the raw CST on a separate entrypoint and keep seeing the literal placeholder — correct.

## Mechanics

**Default-only lookup** is the whole hermeticity mechanism: `interpolate_env_with(src, &|_| None)` — `${env:X:-d}` → `d`; `${env:X}` (no default) → `Err(var_name)`. Never `std::env` in camel-lint or the LEAN path (diagnostics/loads are a pure function of text; ADR-0069 §13.1).

1. **camel-dsl `yaml.rs`**: new `pub fn load_from_file_with_env(path, lookup: &dyn Fn(&str) -> Option<String>)` — reads capped content, interpolates via the lookup, then `parse_yaml_inner`. `load_from_file` becomes `load_from_file_with_env(path, &|_| None)`. Pre-landing check: no camel-config caller depends on the current non-interpolating behavior (config interpolation is a separate tree-walk resolver — rc-v1sw, do not conflate).
2. **camel-cli `commands/test/runner.rs`**: `load_routes` uses the interpolating loader for both file forms; the inline branch interpolates the re-serialized YAML before `parse_yaml`. Errors map to `doc_error` with discovery's wording. Fix the `:144-146` parity comment (now true).
3. **camel-lint `rules/rschema.rs`**: before `raw_to_json_value`, interpolate a copy with the default-only lookup; a token with no default and no resolution is left literal so the schema type error still flags the real defect. Emit one Info diagnostic per substituted default (span: placeholder value node via `value_span_for`). Interpolator lives in a new `camel-lint/src/env_interpolation.rs` mirror.
4. **Baselines & guards**: corpus fixture with an integer placeholder field + RON baseline update (zero-false-positives gate); one LSP session test (no ERROR diagnostic on placeholder doc); guard test for commented-with-default.

## Risks

- rc-ayke widening (accepted, D4). · Corpus baseline churn (mechanical). · Mirror drift between the two interpolator copies (SYNC annotation + a parity unit test covering resolvable and defaulted tokens only — `interpolate_env_with` returns `Err` on unresolved, so lint diverges at policy level: the mirror covers the token-match/substitution arm, and leave-literal-on-unresolved is a lint-side **per-token** wrapper around it; when rc-ayke's comment-strip lands, both adopt it — recorded as CROSS-DEP in bd).

## Out of Scope

rc-ayke proper, rc-gykds (openapi rest blocks), rc-v1sw (TOML tree-walk), Option B (StringOrInt + schema regen — rejected: public-contract change, per-field custom schemars, `xtask schema --check` churn), unit-tier `TestDocument` env vocabulary (ADR-0069 §2 territory).
