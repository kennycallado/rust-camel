# Design: audit-fix-metrics-contract

## Approach

Three independent fixes in `crates/services/camel-prometheus/`, all touching
the same crate but different files.

### rc-asm9: Loopback warning

In `service.rs` `start()`, after binding the listener but before spawning the
server task, check `self.addr.ip().is_loopback()`. If false, emit `warn!`
with the address and the ADR-0052 posture message.

### rc-0pyv: Dynamic metric-name cap

In `metrics.rs`:
1. Add `max_dynamic_collectors: usize` field to `PrometheusMetrics` (default 1024).
   This bounds the number of unique dynamic metric NAMES (collector keys in the
   DashMap), not Prometheus time-series (label-value combinations). Each name
   maps to one `CounterVec` or `HistogramVec` collector. The cap applies
   independently to `dyn_counters` and `dyn_histograms` (total bound is
   2 × max_dynamic_collectors).
2. In `record_counter` and `record_histogram`, check the cap BEFORE acquiring
   the DashMap entry guard. The check is:
   `if self.dyn_counters.len() >= self.max_dynamic_collectors && !self.dyn_counters.contains_key(&normalized)`
   If the cap is exceeded, emit `warn!` and return immediately — do NOT insert
   into the DashMap and do NOT call `entry()`. This avoids the DashMap deadlock
   that occurs when calling `len()` while holding an `Entry` guard.
3. Repeated warnings for cap-exceeded observations are emitted every time (not
   suppressed via the `warned` set). Hitting the cap indicates a problem —
   the warning should be visible. The `warned` set continues to suppress
   warnings for name sanitization and label drift, but NOT for cap exceeded.
4. The cap is a best-effort soft bound: under concurrent access, the `len()`
   check and the subsequent `entry()` insert are not atomic, so multiple
   threads may race past the boundary. This is acceptable for a
   defense-in-depth measure — the goal is preventing unbounded growth, not
   exact enforcement. A small overcount under contention is preferable to a
   global lock that serializes all metric registration.
5. Add `with_max_dynamic_collectors(n: usize)` builder method for configuration.

### rc-7zr3: Status update on server task failure

In `service.rs` `start()`, clone `self.status` (Arc<AtomicU8>) into the
spawned task closure. When the server returns `Err`, store `2` (Failed)
before the `warn!` log. On clean shutdown (Ok), do not set Failed.

## Affected crates

- `camel-prometheus` — service.rs (rc-asm9, rc-7zr3), metrics.rs (rc-0pyv)

## Dependencies

- ADR-0052 (diagnostic endpoint exposure posture) — already committed

## Open questions

None.

## Phases

### Phase 1: Metrics contract hardening

- **Goal:** Enforce ADR-0052 posture (loopback warning), bound dynamic
  metric-name cardinality, fix status-on-exit lie.
- **Dependencies:** ADR-0052.
- **Deliverable:** All camel-prometheus lib tests pass; 3 new behaviors
  implemented and tested.
- **Externally visible interfaces:** New `max_dynamic_collectors` config on
  `PrometheusMetrics` (via `with_max_dynamic_collectors` builder method).
- **Exit criteria:** `cargo test -p camel-prometheus --lib` passes with 0
  failures.
