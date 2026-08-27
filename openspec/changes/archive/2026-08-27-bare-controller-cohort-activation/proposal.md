# Proposal: bare-controller-cohort-activation

## Why

bd rc-fkrd: `route_interception_test::skip::recompiled_pipelines_keep_the_same_rules`
times out (30s direct-call drain) consistently on main. Root cause
(proven by instrumented repro + minimal fix experiment): the
rc-jxkj startup-cohort activation barrier (`25115f6a`, merged after
the test was born) parks EVERY pipeline dispatch on
`cohort_rx.wait_for(open)`. The gate is opened only by the
CamelContext lifecycle (`context_lifecycle.rs` →
`RouteOrderingPort::activate_cohort` via the controller actor
handle). `DefaultRouteController` is publicly re-exported
(`camel_core::route_controller::*`) and drivable outside a full
context — `add_route` + `start_route` succeed, consumers accept
exchanges, and dispatch then parks forever. The barrier silently
broke the bare-controller contract; the test is the canary.

## What Changes

- Add `pub fn activate_cohort(&self)` to `DefaultRouteController`
  (`crates/camel-core/src/lifecycle/adapters/route_controller.rs`)
  that opens the shared gate — the same `CohortActivationGate::open`
  the actor handle performs for the context lifecycle. Doc comment
  states the contract: bare-controller consumers must call it after
  starting routes.
- Fix the test: call `controller.activate_cohort()` after
  `start_route` in `recompiled_pipelines_keep_the_same_rules`
  (`crates/camel-core/tests/route_interception/skip.rs`), mirroring
  what the context does when its startup cohort completes.

Excluded: exposing `reset_cohort` (boot-time concern, context-only);
auto-opening in `start_route` (would defeat the barrier's
cohort-of-many-routes semantics on the actor path); changing the
barrier itself; the `CohortActivationGate` type visibility.

## Acceptance criteria

- `cargo test -p camel-core --test route_interception_test` green
  (all 19 tests, the previously-hanging one included, <1s).
- `activate_cohort` is callable from outside the crate (the
  integration test exercises it) and is idempotent (gate contract).
- Existing in-crate barrier regression tests
  (`cohort_activation_regression.rs`) untouched and green.
- Full quality gates green.

## Risk budget

One public method that flips a `watch<bool>` to `true`
(`send_if_modified` — idempotent, allocation-free); zero behavior
change for the CamelContext path (it opens the same shared gate via
the handle). Out of bounds: any change to gate semantics, the actor
path, or dispatch loops.
