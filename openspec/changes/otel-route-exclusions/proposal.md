# Proposal: otel-route-exclusions

## Why

A partner (bd `rc-mggnb`, camel-112, engine 0.59.0, OTLP gRPC) runs
timer-driven polling routes: nearly empty ticks produce about 254k
exchanges/h (about 70 root traces/s) against a few dozen real traces. The
only sampling knob today is a global sampler, which discards real traces in
the same proportion as noise. They need per-route exclusion that keeps a
noisy route's spans out of export while metrics keep counting.

The folded defect `rc-ue6bv` claimed that the unfiltered
`tracing_opentelemetry::layer()` turns the engine's own `tracing` spans
(`pipeline_step`, `error_handler`, `function`) into orphan root traces. The
owner ruled it closed as not reproduced: in tracing-opentelemetry 0.34,
`layer()` is constructed with `NoopTracer`, and production never calls
`.with_tracer(...)`, so the bridge layer exports no `tracing` spans. R4 is
therefore limited to correcting the misleading comment at
`crates/services/camel-otel/src/service.rs:351` and pinning the real
behaviour with a route-level negative-control test (T7). No bridge filter
API, no production layer change and no production tracer wiring are
delivered.

## What Changes

- **R1** `exclude_routes: Vec<String>` under `[observability.otel]`: glob
  patterns matched against the full `route_id`, case-sensitive, default
  empty (current behaviour). An invalid pattern fails config load with
  `CamelError::Config`, even when `enabled = false`.
- **R2** `RouteExcludeSampler` in the inner slot of `LinkAwareSampler`
  (camel-otel `service.rs`). `ParentBased` is NOT replaced: a sampled parent
  still wins, and a linked `split-item` root still follows its origin's
  decision.
- **R3** Metrics unchanged for excluded routes (`camel.exchanges.total`,
  durations, errors). Verified in code; the step-count semantics of
  `camel.exchanges.total` are NOT changed.
- **R4** Correct the false "picks up the global provider" comment at
  `crates/services/camel-otel/src/service.rs:351`. Add T7: one real route
  exchange under the production-equivalent no-op bridge and a real global
  SDK provider exports exactly the SDK route root and step spans, with no
  `pipeline_step`, `error_handler`, `function` or `step` twin, while the
  `fmt` layer still emits the route log lines.
- **R5** Backward compatible: no config change required, empty default.
- **R6** Docs: tracing page and configuration schema table. The repo has no
  `CHANGELOG.md` by design; the release-notes entry is carried by generated
  Conventional Commit notes instead (design D1).

Out of scope: a `bridge_span_filter` API, any production layer change, any
real-tracer production wiring, per-route `tracing: false` YAML flag (M-L
follow-up), tail sampling in-engine, changing what
`camel.exchanges.total` counts.

## Acceptance criteria

- T1-T7 implemented with the exact names and contracts in `tasks.md`;
  T1-T7 green.
- `cargo test -p camel-otel -p camel-config` green under the containment
  rules in `tasks.md`.
- `cargo test -p camel-cli --test feature_profiles` green. Regenerate the
  golden only if the dependency closure genuinely changed, and document why.
- `cargo fmt --check --all`; clippy `-D warnings` on `camel-otel` and
  `camel-config`. All gates, including fmt, clippy, and lint batteries, run
  under the containment rules in `tasks.md`.
- `openspec validate otel-route-exclusions --type change` clean; a MODIFIED
  requirement in `openspec/specs/trace-model` describing the parent-based
  sampler contract.
- No new workspace dependency beyond `glob` (already a workspace
  dependency).

Bd: `rc-mggnb` (primary), `rc-ue6bv` (folded, closed as not reproduced).
