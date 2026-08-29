# Proposal: memory-gauges

## Why

Epic rc-nkzb (runtime observability for leak/leadership diagnostics, demo-team
demand) needs regression visibility into two memory surfaces that are currently
dark:

1. The pinned HTTPS client cache (`PinnedClientCache`, camel-http) was fixed
   twice in August 2026 for client proliferation/leaks (`4e70eb88`, `ff7334d0`)
   with no live signal: after the fix, a regression would be invisible until
   memory pressure reappears. Only a `#[cfg(test)]` build counter exists.
2. jemalloc is the opt-in soak allocator (`camel-cli` feature `jemalloc`,
   rc-vnm8 soak plan) with the `stats` feature enabled expressly for
   observability — but nothing reads those stats. Leak diagnosis during soak
   requires allocated/resident/active/mapped time series.

Both gaps were flagged as the first wave of rc-nkzb by the 2026-08-29 backlog
triage (e_gpt): direct regression visibility for the recently fixed leak
(rc-u4qz) and live-vs-retained allocation distinction (rc-0sxi).

## What Changes

- camel-http `PinnedClientCache` gains per-call hit/miss accounting with a
  per-call init-executed flag (exactly one miss per client build under
  single-flight, hits for waiters) and live visibility: the cache stores the
  late-bound `MetricsHandle` (ADR-0066) plus its owning `HttpComponentKind`
  enum at construction, and every `get_or_build` access emits
  hit-or-miss (+1) and the approximate entry count through the wired
  collector — all cache producers (steady path and SSRF redirect hops)
  inherit instrumentation from the single choke point.
- camel-api `MetricsCollector` gains dedicated typed methods following the
  `set_queue_depth` precedent (closed label sets, no open strings):
  - `set_pinned_client_cache_size(component, entries)`
  - `increment_pinned_client_cache_hit(component)` / `_miss(component)`
  - `set_allocator_memory(stat, bytes)` with `AllocatorStat` enum
    (`Allocated`/`Resident`/`Active`/`Mapped`)
- camel-prometheus registers the four families
  (`camel_pinned_client_cache_size`, `camel_pinned_client_cache_hits_total`,
  `camel_pinned_client_cache_misses_total`, `camel_allocator_memory_bytes`).
  The OTEL backend documents these as no-op (Prometheus is the diagnostics
  surface for rc-nkzb; OTEL parity is out of scope).
- camel-cli's `run` command (and only it — the sole binary) samples jemalloc
  stats every 5 s under `#[cfg(feature = "jemalloc")]`: the new optional
  `tikv-jemalloc-ctl` dependency, epoch advanced per sample, MIBs initialized
  once, read failures warn-and-retry, never aborting the run.

## Impact

- **New metric families**: four, all closed-set labels (`HttpComponentKind`
  and `AllocatorStat` enum derivations) — lint-metric-labels-clean by
  construction; an invariant test pins the component label to exactly
  `camel-http`/`camel-https`.
- **Codecs**: camel-api (trait methods + `AllocatorStat` + default no-ops +
  Composite forwarding), camel-prometheus (families), camel-http (cache
  instrumentation), camel-cli (run-command sampler + optional dep).
  camel-component-api untouched.
- **No behavior change**: all metrics are additive observability; no lever
  added (precedent: queue-depth sampler is unconditional); sampler failures
  degrade to a warning, never an abort.
- **Specs**: `component-metrics-emission` gains two ADDED requirements
  (pinned-cache visibility, allocator-memory visibility), scoped to the
  wired Prometheus collector.
- **Bd**: rc-u4qz, rc-0sxi (both P2, claimed). Soak context rc-vnm8 unaffected.
