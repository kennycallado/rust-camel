# Proposal: bench-missing-cells

## Why

The comparative benchmark suite (`benchmarks/`, v4 published) outran its paper
backlog, but a pre-flight papal investigation (2026-08-29, bd rc-fz2) found
three genuinely missing measurement cells plus two reporting gaps:

1. **No payload-size axis.** Every scenario today exchanges single tiny
   bodies ("ping" request / "bench" 5-byte M3 body, hardcoded in
   `benchmarks/harness/loadgen/src/cli_runtime.rs`). v4 itself flags the gap:
   "Real workloads with larger payloads (JSON deserialization, validation)
   would shift the bottleneck and may change relative rankings"
   (`docs/benchmarks/2026-07-22-benchmark-v4.md:436`).
2. **No JSON-processing route.** No benchmark fixture exercises
   unmarshal→validate→transform→marshal — the most common integration shape
   (zero JSON route steps across `benchmarks/scenarios/`, verified by grep).
3. **No one-to-many EIP surface.** `benchmarks/CONTEXT.md` §7 flags
   "should T2 be extended to split + aggregate" as the open follow-up; it
   requires a multi-message harness shape the suite lacks.
4. **Ratios carry no uncertainty.** v4's "Key ratios" table publishes bare
   multipliers; BCa exists per-cell (`loadgen/src/bca.rs`) but no CI-on-ratio
   is computed anywhere.
5. **No measured cost for the observability stack.** The memory-gauges
   change (6a01c2f2, per-request metric emission) raised "does it cost
   anything?" — answerable today only with an A/B run, which nobody has run.
6. **CI runs nothing.** `.github/workflows/ci.yml` has zero bench steps; the
   suite is manual-only.

## What Changes

Extend the EXISTING suite (no new harness paradigm):

- **Payload axis**: `bench-loadgen` gains `--payload-size <bytes>`
  (deterministic body generation) for T3 M2/M3; T2-style fixtures gain a
  `BENCH_PAYLOAD_BYTES` env hook. Classes: 1 KiB, 32 KiB, 256 KiB, 1 MiB.
- **New scenario `t2-json`**: timer-driven Protocol-B route doing
  unmarshal(json)→jsonpath validate→transform→marshal(json), the suite's
  3 contenders / 6 artifact fixtures, marker-emitting per suite contract.
- **New scenario `split-aggregate`**: fixed-count one-to-many cycle via
  two routes joined through `direct:` (set_body JSON array of N=100 →
  split → fragments to direct:agg-in → aggregate completion_size=100 →
  marker asserting item count), same artifact set.
- **Ratio CIs**: new `aggregate-ratios` loadgen subcommand computing paired
  bootstrap CIs on throughput ratios from `per_round_means`; output applied
  to published v4 data and to any new run.
- **Gauge-overhead A/B (lever study)**: T3 http-server rust-cli run with
  the same Prometheus backend in both arms; metric families
  exchange/duration/components enabled vs master `enabled=false`
  (ADR-0066 levers), 5 rounds × 30 s per arm, ratio+CI reported.
- **CI subset**: new `bench-smoke` job running criterion quick-mode subset
  (camel-bench `pipeline` + `body_coercion`) — NOT the container matrix.

Excluded (tracked separately): concurrency/route-count scaling (rc-pfuy),
fault-injection (rc-7oy6), allocations-per-request (rc-3emb, blocked by
rc-i9f9), containerized-dep cells (T5/T6 open-if).

Affected crates: `benchmarks/harness/loadgen` (workspace member, excluded
from default-members), fixture binaries under `benchmarks/scenarios/`,
`.github/workflows/ci.yml`, `benchmarks/COVERAGE.md`, `docs/benchmarks/`.

## Acceptance criteria

- Payload classes 1 KiB/32 KiB/256 KiB/1 MiB selectable per run
  (invalid sizes rejected), reported per class, artifacts byte-equivalent
  (golden SHA-256 digests, fixture-side size asserts).
- `t2-json` and `split-aggregate` fixtures pass the harness marker contract
  for all contenders (`BENCH_ROUTE_READY` exactly once, body-suffix proves
  branch execution where applicable).
- Ratio CIs: `aggregate-ratios` emits paired-bootstrap `RATIO point lo
  hi` rows (same-run validation, M3 only); v4 published ratios re-stated
  with CIs in docs.
- Gauge A/B produces a throughput ratio with CI (families-on vs
  families-off on T3×M3, 5 rounds × 30 s per arm, backend constant).
- CI `bench-smoke` job green on ubuntu, timeout-minutes: 10, quick-mode
  args validated by one local run.
- `benchmarks/COVERAGE.md` matrix updated for every new cell/axis.

## Risk budget

Acceptable: benchmark-only code, no production-crate changes; harness churn
inside `benchmarks/`. Out of bounds: touching `crates/*` runtime behavior,
changing published v1-v4 numbers, adding CI jobs that need containers or
> 10 min. Risk of payload-axis matrix explosion is contained by measuring
new axes on rust contenders + one JVM/native reference, not the full matrix.
