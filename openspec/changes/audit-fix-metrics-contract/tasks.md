# Tasks: audit-fix-metrics-contract

## Phase 1: Metrics contract hardening

- **Goal:** Enforce ADR-0052 posture (loopback warning), bound dynamic
  metric-name cardinality, fix status-on-exit lie.
- **Dependencies:** ADR-0052.
- **Deliverable:** All camel-prometheus lib tests pass; 3 new behaviors
  implemented and tested.
- **Externally visible interfaces:** New `max_dynamic_collectors` config on
  `PrometheusMetrics` (via `with_max_dynamic_collectors` builder method).
- **Exit criteria:** `cargo test -p camel-prometheus --lib` passes with 0
  failures.

## Task 1: Loopback bind warning + status-on-exit fix

### Files

- `crates/services/camel-prometheus/src/service.rs` (modified)

### Steps

1. Add `tracing-test = { workspace = true, features = ["no-env-filter"] }` to
   `[dev-dependencies]` in `crates/services/camel-prometheus/Cargo.toml`. This
   workspace dep is already declared (precedent: camel-kafka, camel-sql, etc.).

2. In `start()`, store `self.status.store(1, Ordering::SeqCst)` BEFORE
   `tokio::spawn` (move the existing `self.status.store(1, ...)` from line 133
   to before the spawn block at line 119). This prevents a race where immediate
   task failure stores Failed, then start() overwrites with Started.

3. In `start()`, after the listener bind succeeds and before storing Started,
   add the loopback check:
   `if !self.addr.ip().is_loopback() { warn!(addr = %self.addr, "prometheus metrics endpoint bound to non-loopback address; endpoint is reachable from all interfaces without application-layer restriction (ADR-0052)"); }`

4. Clone `self.status` into the spawned task closure before `tokio::spawn`:
   `let status = Arc::clone(&self.status);`. Inside the closure, when the
   server returns `Err`, add `status.store(2, Ordering::SeqCst);` before the
   existing `warn!` call. The closure captures `status` by move.

5. In `stop()`, after the server task is joined/aborted, store
   `self.status.store(0, Ordering::SeqCst)` (Stopped) — this already exists
   implicitly but verify the status transitions to Stopped after clean
   shutdown, NOT Failed.

- name: `loopback_bind_emits_no_warning`
  setup: `let mut service = PrometheusService::new("127.0.0.1:0".parse().unwrap());`
  action: Call `service.start().await` then `service.stop().await`
  assert: `assert!(service.start().await.is_ok())`. Annotate test with `#[traced_test]`,
  then `assert!(!logs_contain("non-loopback"))` — confirms no non-loopback warning.
  Requires `tracing-test = { workspace = true, features = ["no-env-filter"] }` in dev-dependencies.
  command: `cargo test -p camel-prometheus --lib loopback_bind_emits_no_warning`
  expected: pass after implementation

- name: `non_loopback_bind_emits_warning`
  setup: `let mut service = PrometheusService::new("0.0.0.0:0".parse().unwrap());`
  action: Call `service.start().await` then `service.stop().await`
  assert: `assert!(service.start().await.is_ok())`. Annotate with `#[traced_test]`,
  then `assert!(logs_contain("non-loopback"))` — confirms the warning IS emitted
  with the address.
  command: `cargo test -p camel-prometheus --lib non_loopback_bind_emits_warning`
  expected: pass after implementation

- name: `server_task_error_sets_status_failed`
  setup: Create a `PrometheusService`, start it, get `status_arc()`. Then simulate
  server failure by directly storing the error status as the task closure would:
  `let status = service.status_arc(); status.store(2, Ordering::SeqCst);`
  action: Call `service.status()`
  assert: `assert_eq!(service.status(), ServiceStatus::Failed)`
  command: `cargo test -p camel-prometheus --lib server_task_error_sets_status_failed`
  expected: pass after implementation

- name: `clean_shutdown_does_not_set_failed`
  setup: `let mut service = PrometheusService::new("127.0.0.1:0".parse().unwrap());`
  action: Call `service.start().await`, then `service.stop().await`
  assert: After stop, `assert_ne!(service.status(), ServiceStatus::Failed)` —
  status should be Stopped (0), not Failed (2)
  command: `cargo test -p camel-prometheus --lib clean_shutdown_does_not_set_failed`
  expected: pass after implementation

- name: `status_started_before_spawn`
  setup: `let mut service = PrometheusService::new("127.0.0.1:0".parse().unwrap());`
  action: Call `service.start().await`
  assert: `assert_eq!(service.status(), ServiceStatus::Started)` immediately
  after start returns
  command: `cargo test -p camel-prometheus --lib status_started_before_spawn`
  expected: pass after implementation

### Acceptance

- `cargo test -p camel-prometheus --lib` passes with 0 failures
- `cargo clippy -p camel-prometheus -- -D warnings` exits 0
- `cargo fmt --check` passes

- [ ] task-1

## Task 2: Dynamic metric-name collector cap

### Files

- `crates/services/camel-prometheus/src/metrics.rs` (modified)

### Steps

1. Add `max_dynamic_collectors: usize` field to `PrometheusMetrics` struct
   (line 67). Initialize to `1024` in `PrometheusMetrics::new()` (line 172).

2. Add `pub fn max_dynamic_collectors(&self) -> usize` getter method.

3. Add builder method `pub fn with_max_dynamic_collectors(mut self, n: usize) -> Self`
   that sets `self.max_dynamic_collectors = n` and returns `self`.

4. In `record_counter` (line 248), BEFORE the `use dashmap::mapref::entry::Entry;`
   and `match self.dyn_counters.entry(...)` call (line 261), add the cap check:
   ```rust
   if self.dyn_counters.len() >= self.max_dynamic_collectors
       && !self.dyn_counters.contains_key(&normalized)
   {
       tracing::warn!(
           name,
           cap = self.max_dynamic_collectors,
           "dynamic counter cap exceeded; observation dropped"
       );
       return;
   }
   ```
   This check runs before acquiring the entry guard, avoiding deadlock.

5. In `record_histogram` (line 310), apply the same pattern with `dyn_histograms`
   before the `match self.dyn_histograms.entry(...)` call (line 332).

### Tests

- name: `default_max_dynamic_collectors_is_1024`
  setup: `let metrics = PrometheusMetrics::new();`
  action: Call `metrics.max_dynamic_collectors()`
  assert: `assert_eq!(metrics.max_dynamic_collectors(), 1024)`
  command: `cargo test -p camel-prometheus --lib default_max_dynamic_collectors_is_1024`
  expected: pass after implementation

- name: `dynamic_counter_within_cap_accepted`
  setup: `let metrics = PrometheusMetrics::new().with_max_dynamic_collectors(3);`
  action: Call `metrics.record_counter("a", 1.0, &[])`, `metrics.record_counter("b", 1.0, &[])`,
  `metrics.record_counter("c", 1.0, &[])`
  assert: No error, all 3 accepted. Verify by checking `metrics.gather()` output
  contains all 3 counter names.
  command: `cargo test -p camel-prometheus --lib dynamic_counter_within_cap_accepted`
  expected: pass after implementation

- name: `dynamic_counter_exceeding_cap_rejected`
  setup: `let metrics = PrometheusMetrics::new().with_max_dynamic_collectors(2);`
  action: Record `"a"`, `"b"` (fill cap), then `"c"` (over cap)
  assert: `metrics.gather()` output contains `"a"` and `"b"` but NOT `"c"`.
  Annotate with `#[traced_test]`, then `assert!(logs_contain("cap exceeded"))`.
  command: `cargo test -p camel-prometheus --lib dynamic_counter_exceeding_cap_rejected`
  expected: pass after implementation

- name: `dynamic_histogram_exceeding_cap_rejected`
  setup: `let metrics = PrometheusMetrics::new().with_max_dynamic_collectors(2);`
  action: Record histograms `"a"`, `"b"` (fill cap), then `"c"` (over cap)
  assert: `metrics.gather()` output contains `"a"` and `"b"` but NOT `"c"`.
  Annotate with `#[traced_test]`, then `assert!(logs_contain("cap exceeded"))`.
  command: `cargo test -p camel-prometheus --lib dynamic_histogram_exceeding_cap_rejected`
  expected: pass after implementation

- name: `existing_counter_still_works_after_cap_hit`
  setup: `let metrics = PrometheusMetrics::new().with_max_dynamic_collectors(2);`
  action: Record `"a"` (value=1.0), `"b"` (fill cap), `"c"` (rejected), then
  `"a"` again (value=5.0)
  assert: `metrics.gather()` output shows `"a"` with total value 6.0 (1.0 + 5.0).
  command: `cargo test -p camel-prometheus --lib existing_counter_still_works_after_cap_hit`
  expected: pass after implementation

### Acceptance

- `cargo test -p camel-prometheus --lib` passes with 0 failures
- `cargo clippy -p camel-prometheus -- -D warnings` exits 0
- `cargo fmt --check` passes
- Cap check runs BEFORE entry guard acquisition (no deadlock risk)
- `cargo xtask lint-unwrap` passes

- [x] task-2
