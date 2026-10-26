# Benchmarks

Comparative benchmark suite for rust-camel. This README uses the public
vocabulary only: run (corrida), scenario (escenario), contender
(contendiente), date (fecha), record (registro). Technical depth lives in
[harness/CONTEXT.md](harness/CONTEXT.md). To add a new scenario or a new
contender, follow the recipes in
[scenarios/README.md](scenarios/README.md).

## What a run is

A run measures one or more scenarios on this host. Each scenario is a
workload; each contender is a system under test. A run produces raw
measurements for every scenario/contender cell.

## How to start a run

```bash
bench run --scenarios=<scenario>[,<scenario>...] [flags...]
```

`bench run` passes through to the harness with identical flags and
environment. See `bench help` for subcommands.

For the full canonical record (all scenarios AND all metrics):

```bash
bench run-all
```

A bare `run-all` defaults to the full metric set `m1+m2+m3+m4` (run.sh
METRIC default, bd rc-awyoj): every scenario runs AND every metric arm
(m1 cold-start, m2 warm p99, m3 sustained throughput, m4 memory
growth) is measured. Explicit `--metric=` subsets remain a developer
knob for focused runs. See [runner/RUNBOOK.md](runner/RUNBOOK.md) §4.

## Where records live

Completed runs land in `records/`, one directory per run, indexed by date
in `records/index.json`. Each record holds the run's summary and per-cell
measurements.

## Where contenders live

`contenders/` holds the consolidated contender builds: the single
`rust-camel-lib` crate and the shared node runtime dir (one
`node_modules` for all node contenders).

## Guards

A performance regression on a hot path costs every message. These are the
surfaces where a guard belongs:

- language eval — per-eval property conversion (bd rc-m01r9 Rhai
  `property()`)
- exchange clone and property access
- step dispatch
- direct hop
- split/aggregate, per fragment
- body coercion
- bridge decode

Guard-first rule:

- A performance bug lands a FAILING guard before its fix.
- Only ratio and complexity guards gate PRs. Absolute-time guards never gate
  because machines differ.
- Envelope constants cite their bd ticket. Changing one needs owner approval.

Existing guard: `cargo bench -p camel-language-rhai --bench property_scaling`
(bd rc-m01r9), source at
[benches/property_scaling.rs](../crates/languages/camel-language-rhai/benches/property_scaling.rs).
It measures the W and D shapes at K=100 and K=5000 and reports their ratio R.
Envelopes: read `R <= 2.0`, mutating `R <= 3.0`. Until the phase-2 fix lands,
CI bench-smoke runs it with `RHAI_GUARD_EXPECT_FAIL=1`, so it passes only
while the gate fails.

Evidence caveat: `rc-0jkf2` and `rc-087hq` (both DEFERRED) must land before
bench records are cited as release evidence.

Fixtures: the rc-m01r9 guard uses synthetic properties because no external
fixture is shared. Real-workload numbers live in bd.

## Where technical depth lives

Methodology and design decisions are documented in
[harness/CONTEXT.md](harness/CONTEXT.md) — read it before interpreting any
record.
