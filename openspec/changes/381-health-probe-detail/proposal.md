# Proposal: 381-health-probe-detail

## Why

In the camel-112 incident the partner saw `/readyz` answer
`{"status":"Degraded","services":[],"timestamp":"..."}` while the JMS broker
status was stale. The body named no service and gave no reason, which is part
of why the flapping took a long thread to explain. Root cause verified in
`crates/camel-health/src/lib.rs`: `/readyz`, `/healthz` and `/startupz` call
the bare-status `HealthSource` methods and build
`HealthReport { status, ..Default::default() }`, so the per-service detail that
`/health` serves is dropped at the probe endpoints. A Degraded probe answer
must never return an empty `services` array when a service actually failed or
degraded. Bd: rc-nd04o.

## What Changes

- `camel-api`: additive defaulted `HealthSource` methods `liveness_report()`,
  `readiness_report()`, `startup_report()` returning `HealthReport`; defaults
  reproduce today's status-only shape so every existing implementor compiles
  unchanged.
- `camel-config` (`ContextHealthSource`): override `liveness_report()` and
  `readiness_report()` to serve the per-service detail the source already
  holds (lifecycle vector + `HealthCheckRegistry`), derived from a shared
  readiness evaluation so the aggregate status cannot drift from the detail.
  `startup()` stays unconditionally Healthy; `startup_report()` stays on the
  trait default.
- `camel-health`: probe handlers await the `*_report()` methods. Status-code
  mapping (200 Healthy/Degraded, 503 Unhealthy), field names, and fail-closed
  timeout behavior unchanged. Timeout paths synthesize a `probe-timeout`
  `ServiceHealth` entry like `/health` already does.
- Docs: endpoint contract (`/readyz` vs `/health` purpose, body carries the
  reason) in `docs/src/operations/health.md` and
  `crates/camel-health/CONTEXT.md`.
- Healthy body shape decided: pass-through — a probe body is exactly the
  report the source produced for that probe domain; detail-capable sources
  list all relevant services even when Healthy; status-only sources (trait
  defaults) yield an empty `services` array for every aggregate status.

Excluded: health semantics, aggregate rules, readiness-relevant service sets,
JMS bridge, producer pool, verbose query parameters, version bumps.

## Acceptance criteria

- Degraded `/readyz` names the degraded service and its message (red/green).
- Handler-timeout probe responses (all three probes) carry a synthetic
  `probe-timeout` service entry, 503, fail-closed (red/green); ordinary
  Unhealthy responses carry the source's own entries and no synthetic one.
- Healthy probe shape tested and documented for both a detail-capable source
  and a status-only default source.
- All serialized field names unchanged: `status`, `services`, `timestamp`,
  and per entry `name`, `status`, `message`.
- `cargo test -p camel-health` plus affected-crate tests pass; full gate
  registry green.

## Risk budget

Wire-format risk is bounded: no field is added, removed, or renamed; the
observable change is that `services` entries are populated where they were
always allowed by the schema. Consumers asserting on probe bodies must be
inventoried (camel-prometheus, camel-test) before completion.
