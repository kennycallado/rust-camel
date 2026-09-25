# Proposal: dotryorig

## Why

When a `do_try` catch block itself fails, the runtime returns the catch
error and drops the original error from every observable surface. Route
authors who catch a business error and then fail inside compensation or
translation code lose the original cause: logs, spans, and the returned
error carry only the catch error. The landed `handled_by` delegate
failure rule (rc-ntpof, mission 248, commit f0478ad9) established the
envelope philosophy for one delegation surface; bd rc-zgbqq tracks the
alignment decision for `do_try`.

An e_opus mini-ruling (2026-09-25, sealed, recorded verbatim in bd
rc-zgbqq) decided the envelope: the catch error stays the main error
(option (b), modified — the original error is preserved in the log
policy and the span, not in an exchange property). The catch block is
route code, not infrastructure: exception translation is a standard
idiom, and the translated error must reach `on_exceptions` kind
matching and HTTP status mapping. The harm today is not which error
wins — it is that the original error vanishes from observability.

## What Changes

- Envelope contract for the do_try catch-block failure path, on BOTH
  runtime arms: the tower `DoTryService` (builder API) and the
  `DoTrySegment` OutcomePipeline (compiled routes):
  - the catch error remains the main error in every disposition;
  - a `warn` log record carries `original_error` and `catch_error`
    structured (message: "do_try catch block failed; catch error
    supersedes original");
  - the do_try span gains an event with the `original_error` attribute
    and an error field recording the catch error;
  - no new `CamelError` variant or field; the catch error is returned
    unwrapped.
- doFinally interplay stays as today on each path (tower service runs
  finally with the catch error as previous; the segment path skips
  finally per ADR-0025 invariant #4). The pre-existing finally
  divergence between the two paths is out of scope.
- Spec delta in the `error-handler` capability: ADD the do_try
  catch-block failure envelope requirement; MODIFY the delegate
  failure requirement's distinguishing paragraph (do_try no longer
  "loses" the original).
- Docs: `docs/src/concepts/error-handling.md` documents the envelope;
  `do_try.rs` module docs cite it.
- Excluded: unifying the tower/segment finally divergence,
  exchange-on-error propagation, any DSL/schema surface change.

## Acceptance criteria

- Ruling contract pinned at unit tier (do_try.rs, do_try_segment.rs):
  disposition-independent catch-error envelope, structured log fields,
  span event.
- Pinned at e2e tier (camel-test): a translation route (try throws
  `Io`, catch throws a domain error) matches route `on_exceptions` by
  the domain kind and maps HTTP status from it; a failed-compensation
  route responds with the catch error while the captured log carries
  the original error.
- Existing catch-failure pins stay green (catch error wins, finally
  runs on the tower path, catch error restored over finally error).
- Mission gates green: fmt, clippy (all 4 legs), camel-core lib tests
  + dsl/cli batteries, lint-unbounded-wait (ratchet 296), schema
  check, doc-build.

## Risk budget

Low. The returned error value is unchanged on every path; the change
is additive observability (one `warn` record, one span event, one
span error mark). Translation routes are unaffected by construction.
The only behavioral surface is log volume, which is bounded by
catch-block failure frequency. Risk accepted: none beyond the
existing do_try semantics.
