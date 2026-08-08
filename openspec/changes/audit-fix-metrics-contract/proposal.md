# Proposal: audit-fix-metrics-contract

## Why

Three security/lifecycle gaps in camel-prometheus, all referenced by ADR-0052
(diagnostic endpoint exposure posture, already committed):

1. **rc-asm9:** Diagnostic endpoints bind to any interface without warning.
   ADR-0052 rule 3 says loopback is preferred and non-loopback binds MUST
   emit `warn!` at startup. The current code binds silently to whatever
   address is configured, including `0.0.0.0`.

2. **rc-0pyv:** `dyn_counters` and `dyn_histograms` DashMaps grow without
   bound. A malicious or buggy caller can exhaust memory by registering
   unbounded unique metric names. Today's 5 callers are bounded, but the
   contract permits unbounded cardinality — a latent ADR-0032 vector.

3. **rc-7zr3:** The spawned server task logs `warn!` and exits on error
   but never updates `self.status` to `Failed`. `Lifecycle::status()`
   reports `Started` while the server is dead. The stale status propagates
   into the health aggregate, hiding the failure from operators.

## What Changes

- **rc-asm9:** Add `warn!` in `PrometheusService::start()` when the bind
  address is not loopback. ADR-0052 posture: auth/TLS are opt-in hooks
  (already documented, no code change), loopback is the safe default.
- **rc-0pyv:** Add a configurable cap (default 1024) on dynamic metric-name
  collectors. The cap bounds the DashMap keys (unique metric names), not
  Prometheus time-series (label-value combinations). It applies independently
  to `dyn_counters` and `dyn_histograms`. When the cap is exceeded, the name
  is NOT inserted into the DashMap — the observation is dropped and a `warn!`
  is emitted. The cap check runs BEFORE acquiring the DashMap entry guard to
  avoid deadlock. The cap is a best-effort soft bound under concurrent access.
- **rc-7zr3:** Clone `status_arc` into the spawned server task. On error,
  store `2` (Failed) before logging the warning.

## Acceptance criteria

- Non-loopback bind emits `warn!` at startup
- Dynamic metric-name collector count is bounded by a configurable cap
- Server task failure updates status to `Failed`
- All existing tests pass

## Risk budget

Low. The cap is additive (default 1024, well above today's usage). The
status fix is a one-line store. The bind warning is a log-only change.
