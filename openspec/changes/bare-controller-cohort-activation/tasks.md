# Tasks: bare-controller-cohort-activation

## camel-core

### Task 1.1: Public cohort activation on DefaultRouteController + canary test repair

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified)
- `crates/camel-core/tests/route_interception/skip.rs` (modified)

**Steps:**
1. In `route_controller.rs`, in the `impl DefaultRouteController` block (first method slot, directly above `pub(super) fn health_registry`), add:
   ```rust
   /// Open the startup-cohort activation barrier (rc-jxkj).
   ///
   /// The CamelContext lifecycle opens this automatically once the startup
   /// cohort completes. Consumers that drive a bare `DefaultRouteController`
   /// (outside a full context) must call this before dispatching
   /// (typically after starting routes), or pipeline dispatch parks
   /// every envelope until the caller's call timeout surfaces as a
   /// failure.
   pub fn activate_cohort(&self) {
       self.cohort.open();
   }
   ```
   (The probe implementation already present in the working tree matches this exactly — verify, do not duplicate.)
2. In `skip.rs`, in `recompiled_pipelines_keep_the_same_rules`, directly after the `controller.start_route("recompile-route").await.expect("route must start");` statement, ensure exactly: `controller.activate_cohort(); // bare-controller: open the rc-jxkj barrier` followed by `controller.activate_cohort(); // idempotent: second call is a no-op` (probe's first line already present — verify; add the second).
3. Run the canary test and the full integration suite (commands below).

**Tests:** (executable spec)
- `recompiled_pipelines_keep_the_same_rules` (existing, repaired): setup = bare DefaultRouteController with mock/direct/seda registry, intercept rules, route added+started, pipeline recompiled+swapped, `activate_cohort()` called twice (second call proves idempotence through the new pub fn — S3 ownership beyond the gate unit tests); action = producer.oneshot("after-recompile"); assert = call returns Ok within the normal timeout (was: 30s `direct:in call timed out`), exchange lands in mock:q, count 1.
  - `command`: `cargo test -p camel-core --test route_interception_test skip::recompiled_pipelines_keep_the_same_rules` — **expected**: passes in <1s. (One-time diagnostic inversion — removing the call → 30s timeout — was already observed during probe validation; do NOT repeat it.)
- Full-suite regression: `command`: `cargo test -p camel-core --test route_interception_test` — **expected**: all 19 tests pass.
- Gate unit coverage (existing, untouched — do NOT modify): `CohortActivationGate::open_is_idempotent` and `opened_resolves_immediately_when_open` in `crates/camel-core/src/lifecycle/cohort_activation.rs` own the idempotence and level-trigger scenarios; `command`: `cargo test -p camel-core --lib cohort_activation` — **expected**: passes.
- Actor-path regression (existing, untouched): `cohort_activation_regression.rs` in-crate module; `command`: `cargo test -p camel-core --lib cohort_activation_regression` — **expected**: passes (context path unchanged).

**Acceptance:**
- `rg -n "pub fn activate_cohort" crates/camel-core/src/lifecycle/adapters/route_controller.rs` shows exactly one definition; no `reset_cohort` or `cohort_gate` public exposure added.
- `rg -n "activate_cohort" crates/camel-core/tests/route_interception/skip.rs` shows exactly two calls (activation + idempotence re-call).
- `cargo test -p camel-core --test route_interception_test` exits 0 (19 tests, none skipped).
- `cargo test -p camel-core --lib` exits 0.
- `cargo fmt --check --all` and `cargo clippy -p camel-core --all-targets -- -D warnings` exit 0.

- [x] 1.1
