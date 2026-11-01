# Design: do-try-error-clear

## Approach

The err field on `Exchange` is the single source of truth for "this exchange
carries an unhandled failure"; the disposition decides who clears it
(ADR-0019). `DoTrySegment` already stores the try error on the exchange before
running the matcher and the catch body so catch predicates, `on_when`, and
`${exception}` / `${exception.message}` can read it. What is missing is the
disposition side of the contract: `Handled` and `Continued` must clear that
error on the exchange they return, because the block is a local error-handling
island (ADR-0025 §island).

Change in `crates/camel-processor/src/do_try_segment.rs`, in the
`PipelineOutcome::Completed(next)` arm of the catch body:

- `ExceptionDisposition::Handled` → `next.handle_error()`. This sets
  `PROPERTY_EXCEPTION_HANDLED` (`CamelExceptionHandled = true`) and then calls
  `clear_error()`, which removes `error` and the exception properties that
  `set_error()` derived from it.
- `ExceptionDisposition::Continued` → `next.clear_error()`. Same clearing, no
  marker (Continued advances the pipeline; it does not claim the exception was
  handled inside this block).
- `Propagate` (and any future variant, matched by the existing `_` arm),
  catch-body `Failed`, `Stopped`, predicate-error, and no-match paths are
  untouched.

`next` is the catch body's output exchange, so any state the catch body
produced (body, headers, properties, extensions) is preserved; only the
residual error state and its derived exception properties are dropped. The
clear is unconditional on this arm, so an error the catch body itself set on
its output is cleared too — a `Handled`/`Continued` exit means the block's
disposition, not the catch body, decides the residual error.

Whether `CamelExceptionCaught` should be retained after a handled catch is a
separate decision (bd `rc-v0aoo` non-goal), and this change DOES change the
observable state: before the fix the property survived a `Handled` or
`Continued` catch because nothing cleared it; after the fix `clear_error()`
removes it together with the error, so steps after the doTry no longer see it.
The spec deliberately does not settle the retention rule; restoring it, if
wanted, is a separate decision tracked outside this change.

### Ordering contract (pinned by tests)

`try` failure → `set_error(err)` → matcher → `on_when` → catch body runs with
the error visible → disposition clears (Handled also marks) → `finally` runs
on a clean exchange → `Completed`. The catch body must keep seeing the error;
clearing before the catch body would break `${exception}` inside it.

### Interaction with the Unit of Work and consumers

`ExchangeUoWLayer` branches on `ex.has_error()` after the route pipeline
returns (`crates/camel-core/src/lifecycle/adapters/exchange_uow.rs:105`). With
the error cleared, a handled `doTry` reaches `on_complete` instead of
`on_failure`. `camel-mail`'s `has_error()` check is left as is: it is correct
once the core contract holds.

### `CamelExceptionHandled` marker

The segment already removes a stale marker on entry. Restoring
`handle_error()` on the `Handled` arm also restores the property that the
deleted `DoTryService` set, which is exactly what bd `rc-ho9j1` asks about. The
property is set for the rest of the route (an inner `doTry` that handled an
error leaves it visible to outer steps) — that is Camel-compatible
`Exchange.EXCEPTION_HANDLED` behavior, and `clear_error()` intentionally does
not touch it (documented in `crates/camel-api/src/exchange.rs`). No consumer
in this workspace reads it for control flow; control flow keys off
`PipelineOutcome`.

## Affected crates

- `camel-processor`: `do_try_segment.rs` disposition arms; three new unit
  tests (`handled_catch_clears_error_and_marks_handled`,
  `continued_catch_clears_error_without_handled_marker`,
  `finally_after_handled_catch_sees_no_error`); post-block `!has_error()`
  assertions added to the existing handled and nested tests.
- `camel-test`: one end-to-end test in `tests/do_try_test.rs` using a YAML
  route with `on_complete: mock:ok` / `on_failure: mock:fail` and a handled
  `doTry`; asserts `ok = 1`, `fail = 0`.
- `openspec/specs/do-try`: new requirement plus scenarios.
- Docs: no statement becomes false. `docs/src/eip/do-try.md:34` and
  `docs/src/concepts/error-handling.md:58-59` already state that `Handled` and
  `Continued` clear the error. No doc edit expected.

## Architecture boundaries

Data plane only. `DoTrySegment` is a `camel-processor` `OutcomePipeline`
(ADR-0025); it calls the existing `camel-api` `Exchange` methods. No DSL,
runtime, component, language, or control-plane change. No public API addition
or signature change.

## Alternatives considered

1. **Clear the error in the catch-body wrapper** — no such wrapper exists;
   the clear belongs to the disposition arm, which is the only place that
   knows whether the error escapes. Rejected.
2. **Clear immediately after the error is set, and re-set it before the catch
   body** — same result, more moving parts, and it would still need the
   disposition branch. Rejected.
3. **Keep the error on the exchange and teach every consumer to inspect
   `PipelineOutcome` instead** — impossible: consumers below the segment (UoW,
   mail) only see an `Exchange`. Rejected.
4. **Clear on `Propagate` too** — would swallow the failure the route must
   surface. Rejected; `Propagate` keeps the error by definition.
