# Design: language-failure-observability

## Approach

One implementation slice; no delivery phases.

**O1 — metric.** `PipelineRuntimeCtx` already carries the registered
`Arc<dyn MetricsCollector>` and the route id. Add a helper
`record_language_eval_failure(metrics, route_id, &CamelError)` in the
observability adapter. Call it from the `PipelineOutcome::Failed(err)` arm of
`run_steps` in `route_compiler.rs`, which is the single route-level step loop
(process steps and segment steps both surface there). The helper:

- returns unless `err` is a top-level `CamelError::ExpressionFailed` whose
  `route_id` equals the current route id (prevents a `to: direct:subroute`
  failure from being counted again in the caller route); errors raised by the
  handler itself do not pass through this arm and are not counted; retries
  inside the handler do not pass through it either, so one exhausted sequence
  counts once;
- calls the existing defaulted `MetricsCollector::record_counter`
  (`language_eval_failures_total`, value 1.0) with labels
  `language`, `route`, `verb`, `class`;
- takes `class` from `ExpressionErrorClass::as_str()` (typed, never text).

No `MetricsCollector` method is added, so NoOp/composite/test collectors keep
compiling; the emission is not gated by metric levers (like the error family).

**O2 — span.** `record_exception` is the shared "record this failure on this
span" seam used by process spans, segment spans, and restarted split spans. Add
to it (independent of the existing `exception.*` fields):

- `camel.expression.class` = `class.as_str()`;
- `camel.expression.position` = `"{line}:{column}"`, only when `position` is
  `Some` — never an empty or `0:0` placeholder.

In the process tracer's `Err` arm also declare and record the local tracing
fields `expression_class` and `expression_position` on the existing `step`
span; tracing drops undeclared fields, so they must be declared in the
`info_span!` macro.

**O3 — camel test.** The typed path already exists: the direct producer yields
`Err(ExpressionFailed)` and the unit runner reports it as a document error
whose text is the typed `Display` (class, verb, step id, route id, and position
when present). No production change; pin it with a regression test.

A handled failure is NOT renderable on the failing assertion row: both
`Handled` and `Continued` dispositions call `handle_error()`/`clear_error()`
before the reply exchange returns, so `exchange.error` is `None` by contract
(`error_handler.rs`, `exchange.rs`). Changing that would alter handler
semantics, which is out of bounds. A handled evaluation failure is therefore
observable through O1 and O2 only. The scenario tier already formats a
propagated producer error into `action-transport-failure` through the same
typed `Display`; it needs no change and is not a deliverable of this change.

Redaction: only `language`, `route_id`, `verb`, `class`, `step_id` and the
position are rendered. The `cause`, `conversion` payload and any exchange data
are never read for labels, attributes, or test output.

## Affected crates

- `camel-core`: observability helper, `run_steps` emission, span attributes and
  tracing fields; tests in the existing span/metrics test modules.
- `camel-cli`: unit-tier regression test pinning the typed `Display` on the
  propagated-failure document error (no production change).
- `camel-api`: no production change; `ExpressionErrorClass::as_str()` and the
  typed `Display` are reused as-is.
- Docs: `docs/src/services/metrics.md` label table,
  `docs/src/testing/camel-test.md` diagnostics.

## Architecture boundaries

Runtime owns route-level observability: emission lives in the camel-core
pipeline/tracer, not in language crates and not in the CLI. Language crates
keep producing typed `LanguageError`s only; handler-owned log levels are
unchanged. camel-test consumes the typed `CamelError` and never re-derives a
class from strings.

## Alternatives considered

- **New `MetricsCollector` method** (`increment_language_eval_failure`). Adds a
  defaulted method but bypasses the metric-label lint and duplicates the
  existing `record_counter` contract; rejected.
- **Emit in `LanguageExpressionEval` carriers.** Would make the language SPI
  own route observability and require threading a metrics handle through every
  language crate; rejected by the boundary rule.
- **Emit in `TracingProcessor` only.** Filter/choice/segment predicate failures
  never pass through a `TracingProcessor` (segments are not wrapped), so the
  metric would silently miss a whole failure family; rejected.
- **Add a FAIL row for a failed exchange.** Flips currently-passing documents
  (no expectations) from exit 0 to exit 1; rejected to preserve exit codes.
- **Keep a handled exchange error for camel test.** Requires changing
  `Handled`/`Continued` handler semantics; rejected.
- **Fix `CamelError::classify()` for `ExpressionFailed`.** Changes existing
  `camel_errors_total{error_type}` values; separate concern, out of scope.
