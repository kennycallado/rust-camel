# Proposal: do-try-error-clear

## Why

A handled `doTry` leaves `exchange.error` set. `DoTrySegment` calls
`current_ex.set_error(err)` before running the catch body
(`crates/camel-processor/src/do_try_segment.rs:154`), but the `Completed(next)`
arm for `ExceptionDisposition::Handled` and `ExceptionDisposition::Continued`
(~lines 197-208) assigns the catch output and breaks out of the catch chain
without clearing it. The exchange therefore leaves the block with
`has_error() == true`.

This is a correctness defect, not a mail quirk (e_opus ruling 2026-11-01,
bd `rc-v0aoo`). Consumers that key off `Exchange::has_error()` rather than the
`Result` shape observe a failure that was already handled:

- `camel-mail` (`consumer.rs:55-58`) counts a handled message as a failed
  attempt and redelivers up to `maxAttempts`, re-running the catch body each
  time and never marking the message seen.
- The exchange Unit of Work fires `on_failure` instead of `on_complete`
  (`crates/camel-core/src/lifecycle/adapters/exchange_uow.rs:105`).
- The tracer `has_error` attribute misfires.

`direct:` only appears to work because its producer inspects `Err` and ignores
the residual error field. The documentation already states the intended
behavior: `Handled` clears the error (`docs/src/eip/do-try.md:34`,
`docs/src/concepts/error-handling.md:58-59`), so the code contradicts the
published contract.

## What Changes

In scope:

- `DoTrySegment`: the `Handled` arm calls `next.handle_error()` (sets
  `CamelExceptionHandled = true` and clears the error); the `Continued` arm
  calls `next.clear_error()`. `Propagate`, failed catch body, no-match, and Stop
  paths are unchanged.
- The ordering contract is pinned by tests: try failure → `set_error` →
  matcher/`on_when` → catch body runs with the error visible (so
  `${exception}` / `${exception.message}` work) → clear on Handled/Continued →
  `finally` runs on a clean exchange → `Completed`.
- Unit tests in `crates/camel-processor/src/do_try_segment.rs`, plus post-block
  `!has_error()` assertions on the existing handled and nested tests.
- An end-to-end UoW test in `crates/camel-test/tests/do_try_test.rs`: a handled
  `doTry` route with `on_complete` → `mock:ok` and `on_failure` → `mock:fail`
  must record exactly one `ok` and zero `fail`.
- A `do-try` spec requirement: "A Handled or Continued catch exits with the
  error cleared", with scenarios for the handled marker and the clean exchange.

Explicitly excluded:

- `camel-mail` and its `has_error()` check stay as they are; the fix restores
  the core contract that check relies on.
- `error_handler`, route compiler, retry policy.
- Retention of `CamelExceptionCaught` (separate decision).
- Builder parity for the `CamelExceptionHandled` property (bd `rc-ho9j1`) is
  observed, not closed.

Affected crates: `camel-processor` (fix + unit tests), `camel-test`
(integration test). Spec: `openspec/specs/do-try`.

## Acceptance criteria

- A `Handled` catch body still observes the error through exchange properties,
  and the block exits `Completed` with `has_error() == false` and
  `CamelExceptionHandled == true`.
- A `Continued` catch exits `Completed` with `has_error() == false` and no
  `CamelExceptionHandled` marker.
- `doFinally` after a `Handled` catch observes a clean exchange.
- A handled `doTry` under a route UoW fires `on_complete`, not `on_failure`.
- `Propagate`, catch-body failure, no-match, and Stop semantics are unchanged.
- `cargo test -p camel-processor`, `cargo test -p camel-test --test do_try_test`,
  fmt, clippy `-D warnings`, the xtask lint suite, rustdoc `-D warnings`, and
  `openspec validate --all --strict` pass.

## Risk budget

Acceptable: the behavior change is observable by any consumer that inspects
`has_error()` after a `doTry` (that is the intent). No public API changes, no
new dependency, no new crate. `CamelExceptionHandled` is now set by segment
execution, which restores the property that the deleted `DoTryService` set.

Out of bounds: changing mail, the error handler, the route compiler, the retry
policy, or `CamelExceptionCaught` retention.
