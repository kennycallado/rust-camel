# Design: jms-health-stale

## Approach

This is one coherent protocol-and-cache fix. Keep the existing probe worker and single-flight CAS admission. Add an owned daemon scheduler that requests refresh at startup and every REFRESH_AFTER_MS. Its callback only admits work; it never does broker I/O. Inject a scheduler seam for deterministic tests. current() reads snapshots only and does not schedule probes. Skip mode does not schedule broker work. Stop the scheduler before existing bounded worker/provider cleanup. Preserve all generation and publication guards.

Add HealthStatus to both protobuf copies: UNSPECIFIED=0, UP=1, STALE=2, UNKNOWN=3, SKIPPED=4, DOWN=5. Add status and age_ms to HealthResponse using unused field numbers. Keep healthy, broker_connected, and message unchanged. HealthSnapshot.toResponse maps every status; age_ms is the stale age and zero otherwise. No version changes.

Use one crate-local status interpretation path for readiness and monitor reporting. Explicit UP and DOWN update last verification; STALE, UNKNOWN, SKIPPED, and unknown future enum numbers do not. UNSPECIFIED is legacy: apply the existing booleans, including unhealthy on healthy=false and Degraded on true/false. No message parsing. Readiness retains its last verified result for the same bridge instance. Before verification, unverified status is Degraded (200). After UP, unverified stays Healthy (200); after DOWN, unverified stays Unhealthy (503). RPC errors/timeouts always return Unhealthy but do not manufacture a verified broker observation. Clear retained verification when the Ready channel is replaced or the slot is absent/not Ready, so a replacement cannot inherit old UP. Concurrent observations must not reorder retained state; serialize readiness observations if needed without holding a pool guard across await.

Monitor logs distinguish observation status from retained broker connectivity. Emit a credential-safe fixed-message status transition with numeric age_ms once when entering STALE, not on each changing age. Never log false disconnected solely because an explicit status is unverified. Successful health RPCs still reset liveness failures and never restart because of broker fields. Preserve legacy transition tests and behavior.

Amend ADR-0086 and affected component documentation to supersede stale-always-Degraded semantics. Consumer transport-listener integration is a recorded follow-up design: future generation-fenced listener events can invalidate or publish DOWN into the same cache; no listener implementation belongs here. Producer idleTimeout remains rc-w06u6.

## Affected crates

- bridges/jms: scheduler, snapshot serialization, deterministic tests, mirrored protocol.
- camel-component-jms: generated protocol, shared observation mapping, readiness memory, monitor transitions and tests.
- Documentation: ADR-0086, CONTEXT-MAP.md, camel-jms/CONTEXT.md, and JMS guide sections that state stale semantics.

## Architecture boundaries

Broker I/O remains in the Java component bridge. Rust component health maps observations to existing service CheckResult types. No Runtime, DSL, Languages, Functions, or generic health service policy changes.

## Alternatives considered

- Consecutive broker-failure debounce: rejected; isolated polls can remain stale forever without producer traffic.
- Reader-triggered refresh or larger TTL: rejected; freshness would still depend on reads.
- Blocking health RPC probes: rejected; broker stalls must not become liveness failures.
- Treat all unverified responses as healthy: rejected; startup is unverified and known DOWN must persist.
- Producer pool tuning or bridge version bumps: outside mission scope.
