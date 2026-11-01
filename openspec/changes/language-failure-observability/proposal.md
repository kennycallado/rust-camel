# Proposal: language-failure-observability

## Why

Mission 339 published the `language-value-boundary` requirements O1/O2/O3 as
contract canon but deferred their implementation (bd rc-5r117):

- no metric records a language evaluation failure with
  `{language, route, verb, class}`;
- the failing step span does not carry the error class or the script position;
- `camel test` can render a generic failure line for a failed exchange instead
  of the typed evaluation error.

Operators therefore cannot count language failures by language/verb/class, and
a failing script step is hard to locate from traces or from a test run.

## What Changes

Implement the three published requirements without changing language
semantics, the error taxonomy, redaction, or handler-owned observability:

- **O1 metric.** Emit `camel_language_eval_failures_total` (labels `language`,
  `route`, `verb`, `class`) once per top-level `CamelError::ExpressionFailed`
  surfaced by the route step loop. Class comes from the typed
  `ExpressionErrorClass`, never from message text. Emission uses the existing
  defaulted `MetricsCollector::record_counter`; no trait change, no implementor
  breakage.
- **O2 span.** Add `camel.expression.class` and `camel.expression.position`
  (`line:column`) attributes to the failing step span (process and segment
  spans), plus the matching local tracing fields. Omit the position attribute
  when the language reports none.
- **O3 camel test.** Pin the typed `Display` on the propagated-failure
  document error with a regression test. A handled failure clears the exchange
  error by contract, so it is observable only through the O1 metric and the O2
  span; no error-retention change and no exit-code change.
- **Tests/docs.** Metric-label tests for Simple, Rhai, JsonPath and XPath;
  span-attribute capture; `camel test` output with class/position/step and a
  redaction sentinel; a metrics label table and `camel test` diagnostics
  documentation.

Explicitly excluded: new log fields or log levels (handler-owned stays),
metrics for non-evaluation failures, error-taxonomy changes, a new crate, and
walking cause chains to reclassify wrapped errors.

## Acceptance criteria

- Each failure increments `camel_language_eval_failures_total` exactly once
  with the four labels; values are operator/config-derived, never exchange data.
- The failing step span carries class and `line:column`; no position attribute
  when absent.
- `camel test` failure output names class, position and step id, and never
  leaks exchange data.
- Language crates still emit no `warn!`/`error!` for evaluation failures.
- `cargo test` for touched crates, fmt, clippy `-D warnings`, xtask lints,
  rustdoc, mdbook test, and `openspec validate --all --strict` pass.

## Risk budget

Acceptable: one extra metric per handled evaluation failure (the evaluation did
fail); no metric for an `ExpressionFailed` wrapped inside another error's cause
or raised by the error handler itself (out of scope, documented); retries
inside the handler do not increment again (one exhausted sequence counts once). Out of bounds: changing `CamelError::classify()`
label values for the existing error family, changing exit codes, or adding
cardinality from exchange data.
