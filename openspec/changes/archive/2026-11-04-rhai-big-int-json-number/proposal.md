# Proposal: rhai-big-int-json-number

## Why

A header or property holding a JSON integer above `i64::MAX` is refused by the
eager inbound snapshot of the rhai language. The refusal fires at snapshot
time, so **every later `script:` step on that exchange fails**, not only the
step that reads the value. This breaks unrelated processing (bd rc-141om).

The value is already representable in the crate: `JsonNumber`
(`crates/languages/camel-language-rhai/src/json/mod.rs`) stores the exact
source token used by `parse_json`. The pre-flight ruling (e_opus, 2026-11-04,
GO-WITH-CONSTRAINTS, design E) binds such integers as the existing `json
number` wrapper. This refines the owner decision 2026-10-02 (sealed Q4, no
lossy float); it does not reverse it.

## What Changes

- Inbound (`json_to_dynamic`): a JSON integer in `(i64::MAX, u64::MAX]` binds
  as `json number` with its exact decimal token — uniformly for body, headers
  and properties, nested or not. No refusal, no float, no string.
- Outbound (`dynamic_to_value`): a `json number` whose token is a plain
  decimal integer in `(i64::MAX, u64::MAX]` converts back to `Value::Number(u64)`
  — the exact inbound image, so untouched values round-trip bit-identically.
  Every other `json number` still refuses with the stable label `json number`
  (rc-qka42 deferral stays).
- Transaction diff: `json number` compares by token, so an untouched entry
  produces no delta and keeps its original value handle.
- Comparisons between `json number` and native scalars keep refusing with
  `type-mismatch` (existing `guard_pair` registrations; no new coercion).
- Spec delta in `language-value-boundary`: replace the "Large unsigned integer
  is refused" scenario, amend "Structured values round-trip natively" and the
  rc-qka42 converter requirement. Rhai only.
- Docs: rhai CONTEXT.md outbound-conversion paragraph; new `CHANGELOG.md`
  fix entry naming the behavior change.

Affected crates: `camel-language-rhai` (behavior) plus one camel-core
integration-test fixture rebuild
(`crates/camel-core/tests/expression_failure_propagation_test.rs` — its
inbound-refusal fixture evaporates) and repo docs (rhai user guide,
rhai CONTEXT.md, new `CHANGELOG.md`).

Explicitly excluded: other languages (simple, jsonpath, xpath); changes to
`parse_json`/`to_json` built-ins; float or string fallbacks.

## Acceptance criteria

- All tests named in the mission acceptance list pass, including the
  regression test `big_header_does_not_fail_unrelated_later_step`.
- A `json number` above `i64::MAX` reports `type_of(x) == "json number"`,
  `x.to_string()` is the exact decimal, and the exact `u64` round-trips.
- Non-`u64`-image tokens (`"1.5"`, `"5"`, `"18446744073709551616"`) still
  refuse outbound with source `json number`.
- `openspec validate rhai-big-int-json-number --type change --json` passes.

## Risk budget

- Acceptable: rhai-only behavior change on a formerly refused input class
  (it used to fail loudly; it now binds and fails later, at the operation).
- Out of bounds: lossy conversions, cross-language widening of `Value`,
  changes to the rc-qka42 host-helper contract.

## Risk notes for the blessing experts

- After this change no serde_json `Value` is refused inbound. Two existing
  tests beyond the mission list use `u64::MAX` as their refusal fixture
  (`conversion_error_precedence_header_before_property`,
  `group_conversion_failure_is_indexed_to_clause_zero`) and are rebuilt in
  this change (see design.md).
- The repo has no `CHANGELOG.md` (release notes are generated from
  conventional commits by `cargo xtask changelog`). The mission requires one
  fix entry, so this change creates the file.
