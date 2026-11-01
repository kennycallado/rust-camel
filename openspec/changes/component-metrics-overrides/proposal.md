# Proposal: component-metrics-overrides

## Why

`[observability.metrics] components` is one global opt-in for every
component's `camel_component_operations_total` series. Operators cannot
enable the family for the components they run while keeping the default
off for the rest. The e_gpt design ruling (2026-11-01) keeps the global
default and adds optional per-component overrides instead of a
per-metric-family matrix: one effective switch per component, errors
always forwarded.

The HTTP component also emits only the producer boundary
(`component="http", operation="request"`). Inbound requests, which the
same component handles for `from: http:` routes, are invisible on the
uniform family.

bd: rc-v5fgv.

## What Changes

- `[observability.metrics.component_overrides]`: a `component -> bool`
  map. Effective switch for component `C` is
  `enabled && override.get(C).unwrap_or(components)`. `components` keeps
  its meaning as the default for components without an override.
  `enabled = false` still suppresses every operational family; error
  forwarding is never gated.
- Override keys are validated as component identities at config load.
  An override naming an unregistered component is inert, not an error
  (registration is runtime state).
- `ComponentMetrics` becomes component-scoped: the identity is fixed at
  construction and `observe(operation, failed)` no longer repeats it.
  `RuntimeObservability::component_metrics(component)` returns the scoped
  handle; late binding, NoOp, composite and test collectors are
  preserved.
- HTTP consumer emits `component="http", operation="consume"` per
  handled inbound request. Outcome is decided at response
  completion/abort, not at envelope handoff; failures also forward
  `e:http:consume`. No URL, host, header, or body labels are added.
- Spec deltas: `metrics-configuration` and `component-metrics-emission`.
  ADR-0066 amendment, `docs/src/services/metrics.md`, and
  `docs/src/configuration/schema.md` follow.

Explicitly excluded: per-family flag matrix, histogram bucket redesign,
series-budget changes, gating the pinned-client-cache metrics, new
component coverage beyond HTTP consume, non-observability config, new
crates.

## Acceptance criteria

- Precedence matrix passes: global off/on x absent/false/true override,
  including `enabled = false` with an override on.
- Errors forward regardless of any lever or override.
- Scoped handle fixes the component identity; NoOp, composite, and test
  collectors still work; producer `http:request` labels unchanged.
- HTTP consumer records `http:consume` success and failure, with
  `e:http:consume` on failure and no new request-data labels.
- Config schema/profile/include behavior is tested and documented.
- `openspec validate --all --strict` passes.

## Risk budget

- API break: `ComponentMetrics::observe` signature changes across
  component crates. Acceptable inside one workspace PR; mechanical.
- Semantics: "consume failure" includes requests rejected before the
  route and aborted replies, while a route-selected 4xx/5xx status is a
  success; documented in the delta spec so dashboards do not assume
  route-only semantics.
- Every response body (buffered or streaming) is metered on completion,
  body error, or early drop, with exactly-once accounting; a client
  disconnect before body completion is a failure.
- No new series beyond `http:consume` and its error label.
