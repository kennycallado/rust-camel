# Direct-hop baseline (Phase 0, Task 0.2)

- Date: 2026-09-04
- Worktree git rev: `37eb504a0f31f6125d17e9ff1c390aa640324e99`
- Crate: `camel-bench`, bench target `direct`, benchmark id `direct_hop`
- Route under test: `direct:hop` with an empty (no-op) step list, consumer
  started through camel-core's route controller via `CamelContext`
  (`register_component(DirectComponent)` → `add_route_definition` →
  `start()`); the bench owns the producer.

## Command

```
cargo bench -p camel-bench --bench direct -- --save-baseline direct-inline-baseline
```

Baseline stored under `target/criterion/direct_hop/direct-inline-baseline`.

## Result

- `direct_hop` median point estimate: **11030.830256821831 ns/iteration**
  (~11.03 µs; criterion 95% CI 10952.021–11167.452 ns)

## Protocol note

- Criterion default sample configuration: 3 s warm-up, 100 samples,
  ~5 s estimated measurement time. No custom sample size, noise threshold,
  or confidence level was set.
- Saved baseline name: `direct-inline-baseline`. Later phases compare with
  `cargo bench -p camel-bench --bench direct -- --baseline direct-inline-baseline`.
- One warm dispatch runs in setup, outside the measured loop, so first-use
  allocations (consumer registration lookup, channel wiring) are excluded.

Note: the bench files were uncommitted at measurement time (dirty tree at 37eb504a); they were committed as 76638b4d. Production code is byte-identical between the two revs, so the recorded median is valid for both.
