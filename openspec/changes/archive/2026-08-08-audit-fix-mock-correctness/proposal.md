# Proposal: audit-fix-mock-correctness

## Why

The camel-mock component has two correctness defects found in the v1.0
quality audit (modules/camel-mock-quality-2026-08-06.md):

1. **Dead feature — fail_fast_error (I1, rc-zx30):**
   `MockEndpointInner::fail_fast_error` is initialized to `None`,
   reset to `None`, and checked via `is_some()` in `poll_ready` and
   `call`, but is never assigned `Some(error)`. The producer always
   returns `Ok(exchange)`. The entire `fail_fast` mode is dead code:
   the flag exists, the check exists, the lock guard exists, but no
   code path ever sets the error. Tests only exercise the success path
   (line 1748: `assert!(inner.fail_fast_error().is_none())`).

2. **Silent data loss — clone_body drops Body::Stream (I2, rc-wy0y):**
   `clone_body` (lib.rs:686) uses `_ => Body::Empty` for
   `Body::Stream`, silently replacing streaming payloads with empty
   bodies when `copy_on_exchange` is enabled. A comment falsely claims
   streams are uncloneable, but `StreamBody: Clone` exists
   (body.rs:90-97) and `Body: Clone` handles `Body::Stream`
   explicitly (body.rs:187). Any test that copies an exchange with a
   streaming body loses the body silently.

## What Changes

- **rc-zx30:** Add `MockEndpointInner::trigger_fail_fast(&self, error:
  CamelError)` to make the dead feature programmatically usable. Wire
  `assert_satisfied()` to set `fail_fast_error` before panicking on
  expectation mismatches (enables concurrent-producer rejection when
  the assertion runs on a different task than the producer). Add tests
  proving: (a) `trigger_fail_fast` → subsequent producer calls reject,
  (b) `assert_satisfied` failure → `fail_fast_error` is `Some`.
- **rc-wy0y:** Add explicit `Body::Stream(s) => Body::Stream(s.clone())`
  arm to `clone_body`. Replace the false comment. Keep wildcard arm as
  safety net for `#[non_exhaustive]` future variants but update comment.
  Add test proving streaming bodies survive `copy_on_exchange`.

## Acceptance criteria

- `fail_fast_error` can be set via `trigger_fail_fast` and causes
  subsequent `MockProducer::call` to return `Err`.
- `assert_satisfied()` sets `fail_fast_error` before panicking when
  expectations fail.
- `clone_body` preserves `Body::Stream` (Arc-shared, single-consumption).
- All existing tests pass; new tests prove both fixes.
- `cargo clippy -p camel-component-mock --all-targets -- -D warnings` clean.

## Risk budget

Low risk — both fixes are in a test-only component (`camel-mock`).
No production runtime behavior changes. The `trigger_fail_fast` method
adds a new public API surface but only on `MockEndpointInner` (a type
used exclusively in test code).

Bd: rc-zx30, rc-wy0y
