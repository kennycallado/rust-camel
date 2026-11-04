# Proposal: rhai-limit-errors

## Why

Every rhai resource-limit trip renders the same generic detail
`evaluation limit exceeded` (`eval_alt_detail`,
`crates/languages/camel-language-rhai/src/lib.rs`), and the execution
timeout renders no detail at all. Operators cannot tell `max-map-size`
from `max-array-size` or `max-operations`, nor see the configured bound.
This is worse than it sounds for maps: rhai counts nested content, so a
flat map of ~2,300 small entries already trips the default
`max-map-size` of 10,000. Reported against 0.65.0 (bd rc-53uro).

Baseline reproduced with the existing test harness (custom bounds):

- operations trip (bound 10): `detail: Some("evaluation limit exceeded")`
- map-size trip (bound 50): `detail: Some("evaluation limit exceeded")`
- timeout trip (50 ms): `detail: None`, class `Timeout`

## What Changes

- `EvalFailure.detail` for rhai limit/timeout failures names the tripped
  limit (lowercase-kebab, matching the `RhaiLimitsConfig` config keys)
  and its configured bound, e.g. `max-map-size 10000 exceeded`.
- The execution-timeout detail names the bound:
  `execution-timeout 5000ms exceeded`.
- Compile-time expression-depth failures (`ParseError.reason`) name
  `max-expression-depth` / `max-function-expression-depth` with the
  resolved bounds.
- Threading only: `ResolvedRhaiLimits` flows into the detail-mapping
  helpers. No public error enum changes, no class changes, no redaction
  changes.
- Out of scope: error categories, `LanguageError` shape, step-wrapper
  text, other languages, new configuration keys.

## Acceptance criteria

- Each limit trip (operations, string size, array size, map size,
  call levels, expression depth, function-expression depth) and the
  execution timeout produce a detail naming the limit and the bound
  (unit tests assert the exact string with custom configured values;
  depth tests trip each depth setting independently and assert the
  exact reason string).
- One test proves the default bound appears (`max-operations 100000`).
- Error class/category is unchanged for every covered path (tests
  assert `Limit` / `Timeout` classes stay intact). Timeout exact-detail
  tests cover all three `Elapsed` mapping sites (expression/predicate,
  grouped `first_match`, mutating).
- Details remain operand-free: no exchange data, no script-operated
  values, no raw rhai payload text (redaction contract, ADR-0012 /
  language-value-boundary).

## Risk budget

- Low: single crate (`camel-language-rhai`), detail text only.
- Acceptable: compound name for the two depth limits rhai cannot
  distinguish at compile time (documented fallback, no invented bound).
- Out of bounds: weakening the redaction contract, changing public
  enums, inventing bounds for limits rust-camel does not configure.

Bd: rc-53uro
