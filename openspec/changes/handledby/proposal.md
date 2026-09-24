# Proposal: handledby

## Why

bd rc-ntpof (external report, camel-cache team; e_opus-sealed design ruling
2026-09-24). Two defects in the route-level `error_handler.on_exceptions`
delegation model:

1. **DSL layout defect (the filed one).** `handled_by` exists only inside
   the `retry:` block (`DeclarativeRedeliveryPolicy.handled_by`,
   compile.rs reads it under `if let Some(retry)`). `max_attempts == 0` is
   rejected (`validate_redelivery_policy`), so every delegation forces at
   least one REAL redelivery — the failing step re-executes with sleep and
   redelivery headers before the delegate runs. Upstream endpoints are
   double-hit on validation errors and timeouts. "Delegate once, retry
   zero" is inexpressible.
2. **Always-Ok delegate contract (the worse one, found during ruling).**
   `send_to_handler` (camel-processor error_handler.rs:647-673) swallows
   ALL delegate failures: a FAILED delegate plus `handled: true` reports
   SUCCESS (`Completed`). The pipeline lies about outcome.

The runtime API is already correct — `ExceptionPolicy.handled_by` sits at
policy level beside `retry`/`on_steps`/`disposition` (camel-api). Only the
DSL layout, compile wiring, and the delegate-error contract are wrong.

## What Changes

- **DSL layout**: move `handled_by` from `retry:` to the clause level
  (beside `kind`/`handled`/`continued`/`retry`). `retry` becomes optional
  and independent. `retry` + `handled_by` COMPOSE: retry first, delegate
  once retries exhaust (Apache Camel `onException().maximumRedeliveries()
  .handled(true).to()` parity, ADR-0019 L34).
- **Delegate failure**: `send_to_handler` returns real `Err`. Every caller
  maps delegate failure to `StepDisposition::Propagate(original)` —
  pipeline outcome `Failed(original kind)`, never `Completed`. Original
  error stays the main error (kind matching and HTTP status stay
  business-tied). Delegate error reported via log-policy system-broken
  (both errors structured) plus span error. No new `CamelError` variant.
- **Typed terminal conflict**: `steps` + `handled_by` on one clause is
  rejected via a typed `ConfigValidationError` variant (detretry/sedaretry
  precedent — never message-text matching). Top-level
  `error_handler.retry.handled_by` is DELETED (`dead_letter_channel` is
  the catch-all delegate).
- **Old layout hard-fails**: `retry: { handled_by: ... }` is a HARD load
  error via `deny_unknown_fields` on the YAML and JSON redelivery models —
  never silently ignored.
- **Tap semantics**: `handled_by` without `handled`/`continued` = side
  effect + original error propagates.
- **Specs**: new `error-handler` capability spec pinning ruling scenarios
  (a)-(h); MODIFIED `dsl` wildcard scenarios (spec.md:609-660 spell out
  `retry:{handled_by}`); ADR-0019 amendment (delegate-failure rule);
  JSON schema generation + `cargo xtask schema --check`.
- **Out of scope**: `do_try` catch semantics stay as-is (catch error
  propagates, original lost — different by design, tracked rc-zgbqq);
  legalizing `max_attempts: 0` (rejected by ruling).

Affected crates: `camel-dsl` (model, yaml, json, compile),
`camel-processor` (error_handler), `camel-api` (ConfigValidationError
variant), `camel-test` (fixtures), schema assets.

## Acceptance criteria

- Zero-retry delegation works: `{kind, handled: true, handled_by}` runs
  the step once, delegates, no `CamelRedelivered` header.
- `retry` + `handled_by` compose; delegate sees redelivery headers.
- Failed delegate + `handled: true` → `Failed(original kind)`, at step,
  `continued`, and security/circuit boundary paths.
- `steps` + `handled_by` → typed load rejection.
- Old `retry:{handled_by}` layout → hard load error (YAML and JSON).
- `handled_by` without `handled` → tap: delegate fires, error propagates.
- All mission gates green (fmt, clippy, schema-check, lib tests, dsl+cli
  batteries, doc-build; schema assets regenerated first).

## Risk budget

Breaking change to the `handled_by` DSL layout is INTENTIONAL and loud
(hard error, not silent ignore). Runtime outcome change (failed delegate
no longer reports Completed) is the fix itself. No changes to retry loop
timing, redelivery counters, or `do_try`. Zone lease: error_handler +
DSL compile.rs + dsl spec — test-runner settle and compile/embed are
untouched.
