# Design: audit-fix-mock-correctness

## Approach

### I1 — fail_fast_error dead feature (rc-zx30)

The `fail_fast` feature on `MockEndpointInner` is architecturally sound
(check-on-send, reject-after-error), but the assignment link is missing.
Two wiring points close the gap:

1. **`trigger_fail_fast(&self, error: CamelError)`** — a new public
   method on `MockEndpointInner` that sets `fail_fast_error =
   Some(error)`. This makes the feature usable for test scenarios that
   inject errors programmatically ("after this point, all sends to this
   endpoint should fail").

2. **`assert_satisfied()` panic-path** — before each `panic!` call in
   `assert_satisfied()`, set `fail_fast_error = Some(...)`. This covers
   the concurrent test pattern: the producer runs on a spawned task, the
   test thread calls `assert_satisfied()` which detects a mismatch, sets
   the error (then panics). The producer's **next** `poll_ready` or
   `call` invocation will see the error and reject. An exchange already
   past the in-`call` error check when the error is set may still record
   `Ok` — the guarantee applies to subsequent invocations only. This is
   the semantics the field name promises (reject-after-error, not
   abort-in-flight).

### I2 — clone_body drops Body::Stream (rc-wy0y)

`clone_body` mirrors the `Body::Clone` impl pattern from
`crates/camel-api/src/body.rs:179-190`. Add the explicit
`Body::Stream(s) => Body::Stream(s.clone())` arm. `StreamBody::clone`
(body.rs:90-97) does `Arc::clone` — all clones share one
single-consumption stream handle. The first consumer wins; subsequent
consumers get `CamelError::AlreadyConsumed`. This matches the
documented semantics in `StreamBody`'s rustdoc (body.rs:36-72).

Replace the false comment ("Streams and future uncloneable variants
fall back to Empty") with an accurate one noting the wildcard arm is a
safety net for future `#[non_exhaustive]` variants.

## Affected crates

- `camel-component-mock`: `trigger_fail_fast` method, `assert_satisfied`
  wiring, `clone_body` stream arm, new tests.

## Architecture boundaries

All changes are within `camel-component-mock`, a test-utility crate.
No changes to `camel-api` (Body/StreamBody already support Clone).
No changes to Runtime, DSL, or production components. The new
`trigger_fail_fast` method is on `MockEndpointInner` — a type whose
doc-comment says it is "used in tests" and which has no production
runtime consumer.

## Alternatives considered

- **Return `Result` from `assert_satisfied` instead of panicking.**
  Rejected: breaking API change that affects every test caller. The
  panic-before-set-error approach is non-breaking and sufficient.
- **Remove the `fail_fast` feature entirely.** Rejected: the feature
  has legitimate use (testing error propagation in pipelines). Wiring
  it is cheaper than removing it.
- **Materialize streams in `clone_body`.** Rejected: `StreamBody` is
  designed for single-consumption Arc-sharing, not deep cloning.
  Materializing would require async I/O (not possible in a sync `fn`)
  and would violate the stream's lazy-evaluation contract.

Bd: rc-zx30, rc-wy0y
