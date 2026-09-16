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

## Where technical depth lives

Methodology and design decisions are documented in
[harness/CONTEXT.md](harness/CONTEXT.md) — read it before interpreting any
record.
