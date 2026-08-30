# Benchmarks

Comparative benchmark suite for rust-camel. This README uses the public
vocabulary only: run (corrida), scenario (escenario), contender
(contendiente), date (fecha), record (registro). Technical depth lives in
[harness/CONTEXT.md](harness/CONTEXT.md).

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

## Where records live

Completed runs land in `records/`, one directory per run, indexed by date
in `records/index.json`. Each record holds the run's summary and per-cell
measurements.

## Where technical depth lives

Methodology and design decisions are documented in
[harness/CONTEXT.md](harness/CONTEXT.md) — read it before interpreting any
record.
