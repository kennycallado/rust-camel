# Mission 473 benchmark record — rhai choice batching pre-change baseline (bd rc-1y2qs)

- run_id: 20261031T191644Z (criterion evidence snapshot; NOT an era-2 harness run)
- git branch: feature/473-rhai-choice-batching
- machine: mission worktree target/, bench profile, TMPDIR=/home/shared/tmp

## What this record is

Raw Criterion median artifacts for the pre-change (`choice-pre`) baseline of the
`camel-processor` choice bench, copied from `target/criterion/` of the mission worktree at
`/home/shared/rust-camel-worktrees/473-rhai-choice-batching`. This is a mission-level
Criterion record, NOT a `benchmarks/harness` era-2 run: it has no contenders/cells roster,
so per `SCHEMA.md` it MUST NOT carry a fabricated `run.json`, and `index.json` (only harness
runs) is intentionally NOT updated.

This is the `choice-pre` baseline for the gate "10-clause same-language grouped median
<= 250us".

Each `criterion/<shape>-pre/estimates.json` is the saved `choice-pre` baseline
(`target/criterion/choice_lang/<shape>/choice-pre/estimates.json`); the
`criterion/<shape>-change/estimates.json` files are the criterion comparison artifacts
against the immediately preceding run of the same worktree.

## Exact commands (in /home/shared/rust-camel-worktrees/473-rhai-choice-batching, TMPDIR=/home/shared/tmp)

```
cargo fmt -p camel-processor
cargo clippy -p camel-processor --all-targets -- -D warnings
cargo bench -p camel-processor --bench choice -- --save-baseline choice-pre
```

## Results (median point estimates, second full baseline run)

Criterion stores estimates in nanoseconds; values below are converted to microseconds.

| shape | median | unit |
|---|---|---|
| rhai_1_match | 15.460 | µs |
| rhai_10_match_first | 12.462 | µs |
| rhai_10_match_middle | 84.343 | µs |
| rhai_10_match_last | 110.889 | µs |
| rhai_10_no_match | 112.629 | µs |
| mixed_10_no_match | 50.977 | µs |

## Reading

- The per-clause snapshot cost is visible across match positions: one clause is ~12-15 µs;
  matching at index 5 costs ~84 µs (6 evaluations) and matching at index 9 costs ~111 µs
  (10 evaluations), i.e. roughly 14 µs per additional Rhai predicate evaluation.
- `rhai_10_no_match` (10 evaluations) is ~113 µs and is the worst-case same-language shape
  for the gate.
- `mixed_10_no_match` alternates Rhai and simple clauses (5 Rhai + 5 simple evaluations) at
  ~51 µs, showing the simple-language clause cost is small relative to one Rhai snapshot.
- Rhai predicate evaluation goes through the production async timeout/`spawn_blocking` path,
  so run-to-run medians vary with host scheduling; the saved `choice-pre` baseline is the
  second full run on an otherwise idle worktree.
