# Direct-hop Phase 3 bench gate re-run (inline fast path live)

- Date: 2026-09-04
- Worktree git rev: `bf8545a4a32fdbef283bde00d47228df579ae5a8`
- Crate: `camel-bench`, bench target `direct`, benchmark id `direct_hop`
- Route under test: `direct:hop` with an empty (no-op) step list, consumer
  started through camel-core's route controller via `CamelContext`
  (`register_component(DirectComponent)` → `add_route_definition` →
  `start()`); the bench owns the producer.
- Change under test: Phase 3 — inline fast path live (producer selects the
  published `InlineRouteDispatcher` when the consumer route is Sequential).
  GATE: ratio (baseline / current) must be ≥ 5.0.

## Inline-path proof (untimed, runs before the criterion loop)

The bench carries a second route, `direct:proof`, whose single step records
`tokio::task::try_id()` into a shared slot on execution. The bench captures
the producer-side task id around one untimed warm dispatch (inside a real
tokio task) and asserts the two ids are EQUAL — inline dispatch executes the
consumer pipeline on the producer's task; a channel fallback would show a
different (controller-pipeline-task) id. The proof panics on mismatch, so
the gate can never silently measure the channel path. This run: proof
passed (ids equal), then the timed loop ran.

## Command

```
cargo bench -p camel-bench --bench direct -- --baseline direct-inline-baseline
```

Criterion compares the current run against the Phase-0 baseline saved under
`target/criterion/direct_hop/direct-inline-baseline`.

## Result

- Baseline `direct_hop` median point estimate: **11030.830256821831 ns/iteration**
  (~11.03 µs; criterion 95% CI 10952.021–11167.452 ns)
- Current `direct_hop` median point estimate: **1185.7774135812397 ns/iteration**
  (~1.19 µs; criterion 95% CI 1180.422–1192.470 ns)
- ratio: 9.3026 (baseline / current) — GATE PASSED (≥ 5.0)

Criterion's own change report for this run: `time: [1.1868 µs 1.1947 µs
1.2035 µs]`, `change: [−89.358% −89.199% −89.011%]` (p = 0.00 < 0.05,
"Performance has improved").

## Protocol note

- Same criterion default sample configuration as Phase 0: 3 s warm-up, 100
  samples, ~5 s estimated measurement time. No custom sample size, noise
  threshold, or confidence level was set.
- Medians and ratio derived from criterion's own artifacts:
  `target/criterion/direct_hop/direct-inline-baseline/estimates.json` and
  `target/criterion/direct_hop/new/estimates.json`
  (`median.point_estimate`, ratio = baseline / current), computed with:

  ```
  python3 -c "import json;b=json.load(open('target/criterion/direct_hop/direct-inline-baseline/estimates.json'))['median']['point_estimate'];c=json.load(open('target/criterion/direct_hop/new/estimates.json'))['median']['point_estimate'];print(b,c,b/c)"
  ```

- Single run, recorded as-is; no re-rolls. Host was carrying background
  agent-harness load (load average ~8) — shared-machine noise expected per
  the phase plan; the measured band (~1.19 µs) matches the Phase-3 smoke
  measurements (1.14–1.16 µs).
- The timed `direct:hop` route and its measurement loop are unchanged from
  the Phase-0 baseline (empty step list, one dispatch per iteration); the
  `direct:proof` route added for the untimed task-id proof is never
  dispatched in the timed loop.

## Attribution reconciliation

The phase-1 decomposition addendum estimated the inline path at 0.4-0.8 µs
(→ ~18x); this run measured 1.19 µs (9.3x) under host load average ~8. The
~0.4 µs gap over the estimate band is consistent with the decomposition's
own cost inventory under load: per-dispatch `tokio::time::timeout` timer
entry, registry mutex, admission lock, and guard stack — each cheap in
isolation, all inflated by the same scheduler contention the addendum
documented for wakeups. The `channel_roundtrip` control (benches/
direct_decompose.rs, retained) gives the per-host reference for reading
this number on other machines.
