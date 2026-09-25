# Design: dotryorig

## Approach

Implement the sealed e_opus ruling (bd rc-zgbqq comment, 2026-09-25)
verbatim: catch error stays the main error; the original error moves
to observability surfaces only.

Two runtime arms own the catch-block failure path:

1. `crates/camel-processor/src/do_try.rs` — tower `DoTryService`,
   the builder-API path. The `Err(failed)` arm (catch pipeline threw)
   keeps `Err(catch_err)` and today runs finally with
   `previous = catch_err`. Change: before the finally call, emit the
   envelope — `tracing::warn!` with `original_error` and `catch_error`
   structured ("do_try catch block failed; catch error supersedes
   original"), a span event carrying `original_error`, and a span
   error field recording the catch error.
2. `crates/camel-processor/src/do_try_segment.rs` — `DoTrySegment`,
   the OutcomePipeline path used by compiled routes
   (control_flow.rs). The `PipelineOutcome::Failed(catch_err)` arm
   keeps its return value and its ADR-0025 invariant #4 behavior
   (skip remaining catches and finally). Change: the same envelope
   (warn log + span event + span error) before returning.

Span mechanics: neither do_try arm creates its own span; both run
under whatever span the surrounding instrumentation entered. The
contract is therefore split by surface: the `warn!` record is
UNCONDITIONAL (the guaranteed observability floor); the span
contributions are best-effort — when `Span::current()` is a real
span (`!span.is_none()`), the arm emits a span event (name "do_try
catch block failed") with the `original_error` field (Display), and
marks the span error field with the catch error via `record_span_error`
(existing helper in `error_handler.rs`, private — becomes `pub(crate)`,
reused by both arms). When no span is active, the log record is the
only additional surface. Unit tests exercise the span path by
entering a test span.

Log level: `warn!`, per the ruling. This also clears the
lint-log-levels gate without an allowlist entry (that gate restricts
`error!(` callsites). Field style matches the rc-ntpof precedent
(`original_error = ?...`, `catch_error = %...`). Redaction safety:
lint-log-redaction flags only URL/endpoint/host-like captures
(`url|uri|endpoint|address|host|...` inside braces); the error fields
carry `CamelError` values in the same style as the landed rc-ntpof
records, which pass the gate. Tests assert field presence on captured
structured records (field lookup), not on formatted message text.

The finally-also-fails case (tower path only — the segment path skips
finally after a failed catch body): `run_finally` already restores
`catch_err` over the finally error. The ruling adds one observability
point: log the finally error at `warn` with `catch_error` and
`finally_error` structured when a previous error is being restored
inside the catch-failure flow.

No `CamelError` variant, field, or wrap changes. No DSL, schema, or
builder surface changes — the envelope is runtime behavior; catch
configuration is untouched.

## Affected crates

- `camel-processor`: `do_try.rs` (envelope on the tower catch-failure
  arm, finally-restore log), `do_try_segment.rs` (envelope on the
  segment catch-failure arm), `error_handler.rs` (`record_span_error`
  becomes `pub(crate)`), unit tests for both arms.
- `camel-test`: e2e — translation route (kind matching + HTTP status
  from the catch error) and failed-compensation route (response is
  the catch error; captured log carries the original error).
- Docs: `docs/src/concepts/error-handling.md` (envelope paragraph in
  the doTry section).
- Openspec: `error-handler` capability delta (ADD envelope
  requirement, MODIFY delegate-failure distinguishing paragraph).

## Architecture boundaries

Runtime-only change inside camel-processor. The DSL, schema, and
builder crates are untouched: both arms read the same `CatchClause`
configuration they read today; only the failure-path observability
differs. Camel-api is untouched (no error-type change). The segment
path keeps the ADR-0025 invariants (local error-handler island, stop
classification); the tower path keeps the ADR-0019 disposition model.
References: ADR-0019 (amendment 2026-09-24, delegate failure),
ADR-0025 (segment invariants), CONTEXT-MAP error-handling vocabulary.

## Alternatives considered

- Original error wins, catch error secondary (rc-ntpof mirror):
  rejected by the ruling — the catch block is user route code;
  translation idioms would never reach `on_exceptions` or HTTP status
  mapping, and the runtime cannot tell an intentional throw from a
  failed compensation.
- Keep behavior, document only: rejected — the original error stays
  invisible in logs and spans; the observability loss is the actual
  defect.
- Original error in an exchange property: rejected — the exchange is
  dropped on the `Err` path; a property would be dead data. Revisit
  only if exchange-on-error propagation lands.
- Wrap the catch error in `ProcessorErrorWithSource`: rejected — it
  would change the error kind and break kind matching.
