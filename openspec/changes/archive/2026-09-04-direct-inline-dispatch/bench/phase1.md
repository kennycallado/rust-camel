# Direct-hop Phase 1 bench re-run (Hook B)

- Date: 2026-09-04
- Worktree git rev: `f7e401931cf0adc8dfbdc7ed4eea39567e78a1e9`
- Crate: `camel-bench`, bench target `direct`, benchmark id `direct_hop`
- Route under test: `direct:hop` with an empty (no-op) step list, consumer
  started through camel-core's route controller via `CamelContext`
  (`register_component(DirectComponent)` → `add_route_definition` →
  `start()`); the bench owns the producer.
- Change under test: Hook B — collapse the channel, dispatch via `ctx`
  (commit `f7e40193`). Informational only, NOT a gate: Hook B removes one of
  two round-trips, so a partial improvement is expected and just recorded.

## Command

```
cargo bench -p camel-bench --bench direct -- --baseline direct-inline-baseline
```

Criterion compares the current run against the Phase-0 baseline saved under
`target/criterion/direct_hop/direct-inline-baseline`.

## Result

- Baseline `direct_hop` median point estimate: **11030.830256821831 ns/iteration**
  (~11.03 µs; criterion 95% CI 10952.021–11167.452 ns)
- Current `direct_hop` median point estimate: **10791.908791950289 ns/iteration**
  (~10.79 µs; criterion 95% CI 10540.553–10991.687 ns)
- Ratio (baseline / current): **1.0221** (~2.2% faster)

Criterion's own change report for this run: `time: [10.716 µs 10.923 µs
11.136 µs]`, `change: [−3.2391% −1.1353% +0.9019%]` (p = 0.28 > 0.05, "No
change in performance detected" at the default significance level).

## Protocol note

- Same criterion default sample configuration as Phase 0: 3 s warm-up, 100
  samples, ~5 s estimated measurement time. No custom sample size, noise
  threshold, or confidence level was set.
- Medians and ratio derived from criterion's own artifacts:
  `target/criterion/direct_hop/direct-inline-baseline/estimates.json` and
  `target/criterion/direct_hop/new/estimates.json`
  (`median.point_estimate`, ratio = baseline / current).
- Single run, recorded as-is; no re-rolls.
## Attribution addendum (decomposition diagnostic, post inter-phase review)

The 1.02x Hook B ratio triggered a decomposition (diagnostic bench
`direct_decompose`, 10 criterion ids). Findings:

- Bench artifacts (producer clone + Exchange::new in the timed closure):
  0.13 us — irrelevant (direct_hop_prebuilt vs direct_hop_ref delta ~0).
- Dominant cost: the envelope channel + cross-task wakeups of
  `send_and_wait` into the controller pipeline task — ~9.2 us measured
  (`channel_roundtrip`), of which ~8.3 us is multi-thread scheduler
  wakeup/migration cost (current-thread rt: 0.92 us — 10x cheaper; this
  host is shared, bare metal will shrink this).
- Controller per-envelope machinery (what the inline path KEEPS: cohort
  gate, strict check, ArcSwap load, clone_inner, CANCEL scope, DrainGuard):
  ~0.6-1.0 us. Note: this route shape (empty steps) takes the plain
  Sequential loop with a single boxed traversal — the three-stack split
  applies only to top-level Aggregate routes.
- Inline-path estimate (admission + barrier + backoff + snapshot +
  pipeline.call, zero task handoffs): 0.4-0.8 us.

Conclusion: baseline/inline ~18x on this host; the >=5x Phase 3 gate is
plausible. `channel_roundtrip` stays as the per-host wakeup-cost control
for interpreting the Phase 3 gate number.
