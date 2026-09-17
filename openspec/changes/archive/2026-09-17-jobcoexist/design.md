# Design: jobcoexist

## Context

All job forms converge on `execute_job`
(`crates/camel-cli/src/commands/job/mod.rs:951`): the argv filesystem path
(line 669), the embedded single-document path (line 788), and the embedded
virtual-store path (line 926). Each passes the loaded `CamelConfig`
unchanged to `CamelConfig::configure_context_with_beans`, which opens the
journal (`crates/camel-config/src/context_ext.rs`), initializes OTel, and
registers Prometheus/health Lifecycles exactly as `camel run` does. The
journal collision happens during context configuration; the listener
collisions happen at `ctx.start()`.

Ruling: e_opus 2026-09-16, final (bd rc-va2jl) — Option D, no flag,
allowlist suppression, all job forms, truthful job manifests.

## Goals / Non-Goals

Goals:

- `camel job` coexists with a live `camel run` sharing the same ambient
  config; two concurrent jobs coexist with each other.
- Fail-closed behavior elsewhere is untouched: parse and validate the
  ambient config first, project second.
- Compiled job artifacts describe their effective runtime truthfully.

Non-goals: job-in-app (rc-u991y); telemetry opt-in flags; ambient
repository suppression; `camel run` or `camel-config` semantic changes.

## Decision: job boot projection at the execute_job seam

One private function, used only by `execute_job`:

```rust
fn job_effective_config(config: &CamelConfig) -> CamelConfig {
    let mut effective = config.clone();
    effective.runtime_journal = None;
    effective.observability = ObservabilityConfig::default();
    effective
}
```

Invoked at the top of `execute_job`, before the beans registry and
`configure_context_with_beans`. All three convergence sites inherit the
projection because they pass through `execute_job`. `camel run`,
`camel test`, library callers, and `camel-config` internals are not
touched.

Why not a config-layer carve-out: `CamelConfig` is the runtime source of
truth; `camel-config` tests pin that a present journal installs a durable
store and enabled OTel registers a Lifecycle
(`crates/camel-config/tests/context_config_test.rs`). A loader-level
carve-out would infect every consumer or require a second loader mode.

### Allowlist is exact

The projection assigns exactly two fields: `runtime_journal` and
`observability`. Everything else — routes, watch, `idempotent_repo`,
`cache_repo`, `log_level`, timeouts, components, supervision, platform,
stream caching, beans, languages, security, binds, datasources, jobs —
survives by construction (the function assigns nothing else). A
structural test asserts this field-by-field; `CamelConfig` has no
`PartialEq`, so the test constructs a fully-populated config, projects
it, and asserts survivor fields equal and the two target fields neutral.

Batch drain safety: `BatchDepthProbe` registers with the shared
`MetricsHandle` and does not depend on any exporter; default observability
keeps the handle. SEDA queue-depth sampling publishes to that handle
regardless of Prometheus/OTel.

### Health listener verification (ruling extra)

Verified on main: `HealthServer` is registered only under
`config.observability.health.enabled` (same `configure_context` block as
Prometheus), and binds in `HealthServer::start`
(`crates/camel-health/src/server.rs`). Health is NOT driven by any
config key outside `observability`. `observability = default()` therefore
kills the health listener too. No parked half-fix needed.

### Manifest truthfulness

`derive_for_store` (`crates/camel-cli/src/compile/manifest.rs`) merges
listeners from document walks and from `scan_config_entry` (config,
include, profile entries) kind-blind. Change: when the artifact kind is
`TrailerKind::Job`, drop listeners contributed by config entries — the
health/Prometheus declarations the projection suppresses at runtime.
Document-derived listeners (REST/MCP blocks in route documents) stay for
both kinds. Route artifacts (`TrailerKind::Route`) list all
config-declared listeners as today. Tests cover both kinds.

### Singleton audit (ruling extra)

After projection a job owns no journal, no OTel providers, no Prometheus
listener, no health listener. Remaining file-owning surfaces are the
ambient repositories (`idempotent_repo`, `cache_repo` with redb
backends), which a job route may intentionally use — they stay, and the
two-concurrent-jobs test uses a config without redb-backed repositories,
proving zero collisions on the default path. The ticket documents this
residual as the ambient-repository exception.

### Test design

1. Structural unit test (in `crates/camel-cli/src/commands/job/mod.rs`
   tests or a sibling test module): fully-populated `CamelConfig` in,
   assert the two fields neutralized and every other field identical.
2. Real-binary coexistence test (`crates/camel-cli/tests/`, beside
   `job_one_shot_test.rs`): fixture config enabling `[runtime_journal]`,
   `[observability.prometheus]`, AND `[observability.health]`. Reserve
   two ephemeral ports (Prometheus and health), write both into the
   config, spawn `camel run`, poll BOTH ports for readiness, then run
   `camel job` in the same directory. Assert exit 0 and the JSON report
   on stdout. Proof of no journal open and no listener bind is
   lock-based: the running server holds the redb journal lock and both
   ports, so any open or bind attempt by the job would fail boot with
   exit 2 — the exit-0 outcome plus report proves neither attempt
   happened.
3. Two-concurrent-jobs test with forced temporal overlap: same fixture
   shape (no redb repositories), but the job route holds each exchange
   for a bounded delay (a worker pipeline that waits before completing),
   so each job runs long enough that the two processes provably overlap.
   Spawn both, wait for both; assert both exit 0 with `Completed`
   reports.
4. Compiled-artifact tests: job artifact with observability-enabled
   embedded config — manifest lists no listeners. For runtime proof,
   pre-bind the configured diagnostic ports in the test process before
   executing the artifact; a successful exit-0 job run then proves the
   artifact never attempted to bind them. Route artifact with the same
   config — manifest lists both listeners (existing behavior pinned).

## Trade-offs

- Unconditional suppression means a future job-telemetry requirement
  needs two separate opt-ins (exporters vs durable journal with an
  explicit non-ambient path) per the ruling; deferred by design.
- Manifest listener lists for job artifacts under-report what the
  embedded config declares; the manifest documents effective runtime,
  which is the operator-relevant truth.

## Decision record

No new ADR: this is a cli-jobs rule (operator-tool vs data-plane
separation, already ruled — archive 2026-09-11-job-ux-reshape design,
`.opencode/fleet/inbox/jobconfig-findings.md`), not a cross-runtime
architecture change. ADR-0075 governs artifact format; the manifest filter
is an application of its truthfulness, not an amendment. ADR-0002 covers
journal value for long-lived recovery, not one-shot ownership.

## Risks

- Coexistence test flakiness from port reuse and server startup time —
  bounded by ephemeral-port reservation before config write and
  readiness polling before the job runs. ADR-0070 forbids bind-read-drop
  port probes and mandates staged bound listeners for in-process port
  acquisition; these fixtures spawn subprocesses that cannot receive a
  staged socket, so they take the documented subprocess exception: the
  live-server test reserves then releases and relies on the loud failure
  mode (the spawned `camel run` exits on bind error, failing the test
  visibly; one retry allowed); the compiled-artifact test holds its
  pre-bound listeners continuously from reservation through execution,
  so no release/re-bind window exists at all.
- Structural drift if `CamelConfig` grows a new process-singleton field —
  the structural test enumerates fields explicitly, so a new field forces
  a conscious allowlist decision.

## Migration Plan

None. No config format, CLI surface, or artifact format changes; v2
manifests regenerate on next compile.

## Open Questions

None — the ruling resolved all four e_gpt open questions (all forms
suppress; no flag now; diagnostic endpoints only; malformed config still
fails loud).
