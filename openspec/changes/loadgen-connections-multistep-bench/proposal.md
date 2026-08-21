# Proposal: loadgen-connections-multistep-bench

## Why

The rc-vdy2 mutex convoy (per-route `Arc<Mutex<>>` serializing every concurrent exchange) reached production undetected because the benchmark harness could not have caught it:

1. `loadgen measure-throughput` has no connection-count knob — the request-loop task count, tokio worker threads, and connection-pool cap are all derived from a single `--workers` flag that defaults to `available_parallelism()`. Published benchmarks therefore never exercised hundreds of concurrent in-flight exchanges on one route.
2. The `http-server` bench route is a single trivial step (`set_body "pong"`), so per-exchange pipeline work — and any per-exchange lock cost — was negligible.

bd rc-vdy2 fixed the convoy; bd rc-qv1x closes this measurement gap so the next serialization regression cannot ship silently.

## What Changes

- `benchmarks/harness/loadgen`: add `--connections` to `measure-throughput`, decoupling concurrent in-flight requests from tokio worker threads. Default keeps today's behavior (`connections = workers`) for backward compatibility.
- Results artifacts embed the concurrency profile (`workers`, `connections`) as greppable JSON fields.
- New `benchmarks/scenarios/multi-step/` fixture: an HTTP route whose per-exchange pipeline work is non-trivial (two rhai scripts + header derivation + branch evaluation), mirroring the demo-team workload shape (rhai + cache + header rewrite) that exposed the RSS/latency symptoms.

Out of scope: re-running or republishing historical benchmark numbers; changes to other loadgen subcommands; new camel components.

## Acceptance criteria

- `loadgen measure-throughput --connections N` drives N concurrent in-flight requests independent of `--workers`; default (flag absent) preserves current behavior exactly.
- Published result JSON contains `workers` and `connections` fields.
- A multi-step scenario exists under `benchmarks/scenarios/multi-step/` with a rust-camel-cli route using at least three distinct in-DSL step kinds (script, header derivation, branch evaluation — no external infra); a preflight request asserts deterministic output through every step; the load phase is log-free per exchange.
- A c16/c300/c1000 comparison on the multi-step scenario produces artifacts from which the c300/c16 and c1000/c16 throughput ratios are computable from greppable JSON fields alone (convoy signature machine-computable; post-fix expectation: flat or sub-linear degradation).

## Risk budget

Acceptable: loadgen-internal refactors to thread the new flag through; scenario YAML tuned for signal quality over realism.

Out of bounds: changing production crate code; altering existing scenario fixtures; benchmark number publication (separate human-run activity).
