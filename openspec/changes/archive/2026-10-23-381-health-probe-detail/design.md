# Design: 381-health-probe-detail

## Approach

The probe detail is lost because the `HealthSource` trait exposes only bare
`HealthStatus` per probe. Pre-flight ruling (e_gpt): extend the trait
additively and pass reports through unchanged.

1. **camel-api** — add three defaulted async methods to `HealthSource`:
   `liveness_report()`, `readiness_report()`, `startup_report()`, each
   returning `HealthReport`. Defaults build
   `HealthReport { status: self.<probe>().await, ..Default::default() }` —
   byte-identical to today's probe bodies. No existing implementor breaks.
2. **camel-config** — `ContextHealthSource` overrides `liveness_report()` and
   `readiness_report()`. `readiness()` and `readiness_report()` derive from
   ONE shared internal evaluation (lifecycle short-circuit first, then
   registry `check_all()`), preserving existing precedence; the report method
   collects the per-service entries from that same result so the aggregate
   status and the named services can never disagree. Lifecycle entries carry
   only a name and an atomic state, so a failed lifecycle entry gets a
   truthful fallback message derived from its state (e.g.
   `"service lifecycle state: Failed"`) instead of `message: null` — the
   body must explain, not just name. `liveness_report()` reports
   lifecycle-vector entries only (liveness must not inherit readiness-only
   failures); Stopped services keep their existing treatment under the
   liveness aggregate. Tests prove the short-circuit: failed lifecycle
   entries keep precedence and the registry is NOT consulted; Stopped
   entries do not change the readiness status. `startup()` remains the trait
   default (Healthy); `startup_report()` is NOT overridden — its Healthy +
   empty shape is documented and tested.
3. **camel-health** — probe handlers await the `*_report()` methods instead
   of the bare probes. On handler timeout they fail closed exactly as today
   (Unhealthy, 503) but synthesize
   `ServiceHealth { name: "probe-timeout", status: Failed, message:
   "<probe> probe exceeded handler timeout" }` and a fresh timestamp,
   mirroring `/health`. `/health` itself is untouched.
4. **Docs** — `docs/src/operations/health.md`: what `/readyz` vs `/health`
   are for, the body now carries the reason, and the Healthy shape rule
   (all services a detail-capable source produced; empty only for
   status-only sources). `crates/camel-health/CONTEXT.md`: probe contract
   section updated likewise.

Healthy body ruling (pre-flight Q2, Option 1): probe bodies share the
`/health` schema and conventions, not identical contents — each probe serves
exactly the report its source produced for that probe domain.

## Affected crates

- `camel-api`: `HealthSource` gains three defaulted methods; rustdoc updated.
- `camel-config`: `ContextHealthSource` overrides two report methods; shared
  readiness evaluator extracted; tests.
- `camel-health`: probe handlers switch to report methods; timeout synthesis;
  red/green tests.
- `docs` (mdbook): `operations/health.md` endpoint contract.

## Architecture boundaries

Contract types stay in camel-api; no runtime behavior changes in camel-core.
The HTTP surface stays in camel-health; the production detail lives where it
already does (camel-config context wiring). camel-platform-kubernetes
continues consuming `readiness()` and is untouched. No new public enum, so
ADR-0049 non-exhaustive posture is unaffected; serde shapes are unchanged.

## Alternatives considered

- Change `liveness()`/`readiness()`/`startup()` return types to
  `HealthReport`: breaks every implementor including out-of-tree ones;
  rejected (pre-flight Q1).
- Have handlers call `health_report()` and filter per probe: conflates probe
  domains — liveness would inherit readiness-only failures (pre-flight Q2
  clarification); rejected.
- Offenders-only bodies (empty on Healthy, filtered on non-Healthy): probe
  bodies diverge from `/health` for the same state and need filtering logic;
  rejected in favor of pass-through.
- Keep probes terse and document `/health` as the diagnostic endpoint: leaves
  the camel-112 failure mode (empty services on Degraded) possible; rejected
  by the mission order.
