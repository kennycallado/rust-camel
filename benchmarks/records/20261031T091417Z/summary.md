# Mission 458 benchmark record — http route registry ArcSwap (bd rc-solgg)

- run_id: 20261031T091417Z (criterion evidence snapshot; NOT an era-2 harness run)
- git_commit after: c707d9946 (final parked HEAD: perf + review-fix commits; branch feature/458-http-registry-arcswap, base c1912e5d4)
- git_commit before: c1912e5d4 (baseline runs on the base via bench-only commits 6f3f2cd1b/238a25ef2 — bench + doc(hidden) probes only, zero production change)
- machine: same host for baseline and after runs (same worktree target/, bench profile)
- supersedes the intermediate record 20261031T090115Z (measured pre-holistic-fix; removed from the branch)

## What this record is

Raw Criterion artifacts (estimates.json + sample.json for the `http-reg-pre` baseline
and the after runs) copied from `target/criterion/` of the mission worktree. This is a
mission-level Criterion record, NOT a `benchmarks/harness` era-2 run: it has no
contenders/cells roster, so per `SCHEMA.md` it MUST NOT carry a fabricated `run.json`,
and `index.json` (only harness runs) is intentionally NOT updated.

Percentile note: sample_size is 30 per bench; p95/p99 are nearest-rank over the 30
per-iteration samples — coarse tails, quoted for shape comparison only, not SLA evidence.

## Probe contract (read before quoting numbers)

The benched read path is the `#[doc(hidden)]` probe `HttpRouteRegistry::probe_lookup`
plus `probe_static_select`. Contract: ONE lock-scope acquisition covering exact+REST,
and one acquisition for static selection — i.e. the probe mirrors the POST-migration
production shape (one `ArcSwap::load()` per request; verified: `dispatch_handler` loads
once and passes the snapshot through `dispatch_static_with`).

Baseline fidelity disclosure (holistic-review finding 4): PRE-migration production took
a separate tokio RwLock read guard for the exact phase and a SECOND one for the REST
phase, while the frozen baseline probe used ONE guard scope covering both. Baseline
rest/miss numbers therefore omit one read-lock acquire/release (~25 ns measured on the
exact phase: RwLock 92.7 ns vs ArcSwap 66.0 ns). Effect on conclusions, with arithmetic:
- exact benches: single-phase — perfectly faithful both sides; no adjustment.
- miss/1: baseline would be ~97.8 + ~26.7 ≈ 124 ns → recorded −29.3% becomes a
  sensitivity estimate of ≈ −44% (illustrative, not a measured production delta).
- rest/500: baseline would be ~5.618 µs + ~0.027 µs ≈ 5.645 µs → recorded +12.0%
  becomes a sensitivity estimate of ≈ +11.5% (regression conclusion unchanged).
All recorded deltas are conservative in the direction of the baseline (wins understated,
regressions slightly overstated); the adjustments above are sensitivity estimates derived
from the net exact-phase difference, not independently measured lock costs. No
re-baselining with a production-shaped probe was performed; the frozen bench code keeps
`http-reg-pre` comparable.

Concurrent-bench label disclosure (holistic-review finding 3): the bench id
`concurrent_readers_1writer` polls EIGHT probe futures via `join_all` on ONE task —
post-migration `probe_lookup` has no await point, so the eight lookups run sequentially
on one thread while the writer loop runs on a separate OS thread/runtime. The metric is
reader-batch wall-time under writer contention (8 lookups per iteration), NOT eight
OS-parallel readers. The per-lookup lock-elimination win is independently shown by the
exact/miss benches. Bench ids are kept stable for `http-reg-pre` comparability; the
doc comments in `benches/registry.rs` state the same.

## Exact commands (all in /home/shared/rust-camel-worktrees/458-http-registry-arcswap, TMPDIR=/home/shared/tmp)

1. Baseline (bench-only commit, before any production change):
   `cargo bench -p camel-component-http --bench registry -- --save-baseline http-reg-pre`
2. After the implementation (commit c707d9946):
   `cargo test -p camel-component-http`
   `cargo bench -p camel-component-http --bench registry -- --baseline http-reg-pre`

## Results (per-iteration times; p50 = nearest-rank median of the 30 samples)

| bench | before p50 | after p50 | Δp50 | before p95 | after p95 | before p99 | after p99 | criterion verdict |
|---|---|---|---|---|---|---|---|---|
| registry_lookup/exact/1 | 92.7 ns | 65.7 ns | -29.1% | 93.7 ns | 66.6 ns | 94.4 ns | 66.6 ns | improved |
| registry_lookup/exact/50 | 92.4 ns | 67.3 ns | -27.2% | 94.3 ns | 68.7 ns | 95.1 ns | 69.8 ns | improved |
| registry_lookup/exact/500 | 93.5 ns | 70.5 ns | -24.6% | 94.0 ns | 72.2 ns | 94.3 ns | 72.6 ns | improved |
| registry_lookup/rest/1 | 119.7 ns | 90.7 ns | -24.3% | 121.2 ns | 94.4 ns | 123.1 ns | 98.7 ns | improved |
| registry_lookup/rest/50 | 693.5 ns | 634.2 ns | -8.5% | 712.2 ns | 654.3 ns | 719.4 ns | 675.1 ns | improved |
| registry_lookup/rest/500 | 5.618 µs | 6.292 µs | +12.0% | 5.777 µs | 6.636 µs | 5.859 µs | 6.722 µs | regressed |
| registry_lookup/miss/1 | 97.8 ns | 69.1 ns | -29.3% | 99.5 ns | 70.7 ns | 103.2 ns | 71.3 ns | improved |
| registry_lookup/miss/50 | 323.5 ns | 287.3 ns | -11.2% | 330.7 ns | 466.5 ns | 334.9 ns | 472.0 ns | no-change |
| registry_lookup/miss/500 | 2.446 µs | 2.397 µs | -2.0% | 2.571 µs | 2.456 µs | 2.881 µs | 2.458 µs | improved |
| registry_lookup/concurrent_readers_1writer/50_routes_8_readers | 7.687 µs | 394.6 ns | -94.9% | 9.083 µs | 408.0 ns | 9.754 µs | 408.2 ns | improved |
| registry_static/select/1 | 88.4 ns | 58.2 ns | -34.1% | 92.1 ns | 58.5 ns | 93.5 ns | 59.8 ns | improved |
| registry_static/select/3 | 99.4 ns | 66.8 ns | -32.8% | 102.3 ns | 67.3 ns | 102.4 ns | 67.4 ns | improved |

## Reading

- Exact legacy lookup (the per-request hot path): **-25% to -29%**, flat ~66-71 ns across
  1/50/500 routes. One `ArcSwap::load()` replaces one tokio `RwLock` read acquire/release.
- Reader batch (8 lookups) with an active writer: **-94.9%** (7.687 µs → 394.6 ns per
  8-lookup iteration). Readers no longer acquire a lock the writer can hold.
- Static mount selection: **-33% to -34%**; production additionally no longer clones every
  mount's PathBuf/String/error-page HashMap per request (clone elimination is NOT visible
  in the selection-only bench; the production win is larger than shown here).
- REST lookup: 1 route -24.3%, 50 routes -8.5%. 500 routes: **+12.0%** (see disclosure:
  ≈ +11.5% sensitivity-adjusted; ~5.6 → ~6.3 µs) — the 500-template scan dominates (~6 µs of
  template matching); in this scan-dominated regime the uncontended RwLock read guard was
  nearly free and the snapshot indirection shows. Absolute cost stays µs-scale; the mission
  sets no threshold/CI gate. Miss/500 flipped sign between after-runs (+3.6% / -2.0%):
  noise regime, no verdict claimed.
- All comparisons p = 0.00 < 0.05 in the final run; shape reproduced across three
  independent after-runs (exact/concurrent/static stable, rest-500 regression reproducible).

## Spec check

`openspec/specs/camel-http/spec.md` pins duplicate consumer-registration rejection
(atomicity, exactly one winner, sibling methods coexisting, restart reclaim) and the
Purpose line's routing behaviors; no requirement pins the registry locking mechanism
(RwLock vs ArcSwap snapshot). Behavior is preserved (full camel-http suite green,
including the duplicate-registration scenarios). Result: **no spec change**.
