# Design: bare-controller-cohort-activation

## Approach

Single-method API addition plus a one-line test repair; the fix was
validated experimentally during diagnosis (test passes in 0.01s once
the gate opens).

`DefaultRouteController` already owns the shared barrier
(`pub(super) cohort: Arc<CohortActivationGate>`); the actor path
opens it through `RouteControllerHandle`'s `RouteOrderingPort` impl
(`route_ordering_impl.rs` — direct shared-state act, deliberately no
actor round-trip). The new method performs the identical direct act
on the same Arc:

```rust
pub fn activate_cohort(&self) {
    self.cohort.open();
}
```

Placed in the `impl DefaultRouteController` block next to
`health_registry()`. It inherits the gate's contract:
`CohortActivationGate::open` is idempotent, allocation-free
(`watch::send_if_modified`), and level-triggered — subscribers
parked in `wait_for(open)` proceed immediately, future subscribers
see the open level.

The test repair mirrors the context's unconditional post-cohort
activation: after `start_route`, call `controller.activate_cohort()`
with a comment naming the rc-jxkj barrier. No other test in the
suite drives a bare controller through envelope dispatch (audited:
`hot_reload/reload.rs` bare usage is `#[cfg(test)]` action-computation
only; benches drive `BoxProcessor` directly, bypassing route
dispatch; in-crate tests reach internals directly).

## Affected crates

- `camel-core`: one `pub fn` on `DefaultRouteController`
  (publicly re-exported surface); one integration-test line. No
  dependency, trait, or behavior change.

## Architecture boundaries

Adapters layer only; no port or application-layer change. The method
exposes exactly the act the application layer
(`context_lifecycle.rs`) already performs through its port — it does
not bypass or duplicate policy, it makes the existing policy
reachable for the documented bare-controller usage. Hexagonal
boundary test untouched (no new cross-layer edge).

## Phases

Single-phase: one coherent slice (method + test + gates).

## Test strategy

- The repaired integration test
  (`recompiled_pipelines_keep_the_same_rules`) IS the regression
  proof: it drives a bare controller through real dispatch and fails
  (30s timeout) without the call, passes in milliseconds with it.
- Existing gate unit tests (`cohort_activation.rs`: idempotency,
  level-trigger) cover the semantics the method delegates to.
- Full suite: `cargo test -p camel-core --test route_interception_test`
  (19 tests) plus the standard gate battery.
