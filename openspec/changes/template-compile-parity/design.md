# Design: template-compile-parity

## Context

Direct routes and templated routes reach `RouteDefinition` through different
entrypoints. Direct: `discovery.rs` threads `(stream_cache_threshold,
SecurityCompileContext)` via `parse_*_with_threshold_and_security` →
`compile_declarative_route_with_stream_cache_threshold`
(`crates/camel-dsl/src/compile.rs:240`). Templated: Pass 2 calls
`materialize_and_compile` (`crates/camel-dsl/src/template/materializer.rs:187`)
which compiles via `compile_declarative_route`
(`crates/camel-dsl/src/compile.rs:232`) — a wrapper that hardcodes
`SecurityCompileContext::default()` and `DEFAULT_STREAM_CACHE_THRESHOLD`.
Same root cause, three symptoms (F1, F3, F9). Four more gaps are independent
(F2, F4, F5, F6). Full audit: bd rc-f2kp.

## Goals / Non-Goals

Goals: parity for context and threshold; complete fail-closed diagnostics;
typed parameters; per-instance identity and reload-correct hash.
Non-goals: templated `rest:`/`mcp:` blocks, OpenAPI visibility of templated
routes (deferred), changing direct-route behavior.

## Decisions

### D1 — Thread context, keep fail-closed (F1, F3, F9)

`materialize_and_compile` gains `(stream_cache_threshold: usize,
security_ctx: SecurityCompileContext)` and calls
`compile_declarative_route_with_stream_cache_threshold` directly. Discovery
Pass 2 passes the same pair the direct branch uses; the `None` threshold
branch folds into the security-aware path so direct and templated routes
share one source of truth. Empty context still fails
(`require_authenticator`); ADR-0033 fail-closed posture is untouched —
security posture only gains reach.

### D2 — Fail-closed, complete diagnostics (F2, F11)

Startup stays fail-closed (ADR-0033): any failing templated spec still aborts
the process. But discovery collects materialization (Pass 2) errors across
all specs before failing, and `DiscoveryError` carries a structured template
error (error class, template id, spec location) instead of the flattened
"invalid template body" string. Today Pass 2 collapses `CamelError::Config`,
`CamelError::RouteError` (e.g. security-required), and every other variant
into `InvalidBody` (`crates/camel-dsl/src/discovery.rs:359-362`) — the
structured error keeps them
distinct. Pass 1 parse-level failures (template definitions, templated-spec
parsing) remain first-abort; aggregation applies to materialization only.

### D3 — Typed parameters (F4)

`TemplateParameterSpec` (camel-api) gains optional `type`:
`string | number | boolean` (default `string` — backward compatible).
Substitution rule: a placeholder occupying a whole scalar node (string value
exactly `"{{p}}"`) substitutes with the declared type (number/boolean as
JSON scalars); a placeholder embedded in a longer string (e.g. `"x{{p}}"`)
interpolates as text and requires `string`. Non-coercible values (number
param holding "abc") are loud validation errors at parameter resolution,
before JSON round-trip. Compatibility note (corrected): the string-quoted
form (`delay_ms: "{{delay}}"`) remains substitutable as TEXT exactly as
before (ADR-0008 string-tree behavior unchanged) — but deserializing that
string into a numeric field was and is a serde error ("data did not match
any variant"); numeric-field population is precisely what `type: number`
adds (finding F4).

### D4 — Identity and hash (F5, F6)

Route identity: `templated.route_id` override is valid only for
single-route templates; a multi-route template with an override fails with a
Config error pointing at per-route ids inside the template body (which may
use parameters). Single-route templates instantiated N times under distinct
overrides yield distinct ids — the secured-proxy dedup pattern.

Hash: `compute_source_hash` for materialized routes hashes the raw template
body + resolved parameter map + effective route id (the id after `route_id`
override is applied). Today the hash covers only the raw body
(`crates/camel-dsl/src/template/materializer.rs:108-113`), so all instances
of a template share one hash; including the post-override id makes
override-only instances hash distinctly, and hot-reload
(`crates/camel-core/src/hot_reload/application/reload.rs:54`) then detects
parameter-value and identity changes per instance.

## Architecture Boundaries

All changes sit in the DSL layer (camel-dsl) plus one camel-api type.
No data/control-plane crossing; no component changes. Registry monopoly
(camel-core compiles steps at route-compile time) is preserved — we only fix
which compile entrypoint the materializer calls.

Relevant ADRs: ADR-0008 (route-template JSON string-tree substitution — D3
is additive to it), ADR-0017/0026 (declarative DSL → RouteDefinition),
ADR-0033 (fail-closed defaults, per-item explicit choice — D3's typed params
are per-item explicit, D2 aggregates diagnostics without weakening the
abort), ADR-0038 (config-channel analog for template param typing).

## Phases

- **Phase 1 — Context parity (F1, F3, F9).** Thread threshold + security
  context; fold `None` branch. Exit: all five rc-f2kp regression tests pass;
  direct-route tests unchanged.
- **Phase 2 — Diagnostics (F2, F11).** Structured template error in
  `DiscoveryError`; aggregate before abort. Exit: mixed file with two bad
  specs reports both; error class preserved; process still exits non-zero.
- **Phase 3 — Typed params (F4).** `TemplateParameterSpec.type`,
  whole-node typed substitution, resolution-time validation. Exit: numeric
  param populates `u64` field; string default unchanged.
- **Phase 4 — Identity + hash (F5, F6).** Override validation; hash includes
  params + id. Exit: N-instance single-route template materializes; param
  change triggers reload.

Each phase is independently shippable; Phase 1 unblocks the secured-proxy
dedup use case.

## Risk / Trade-offs

Typed params add a camel-api surface (additive, defaulted). Hash semantics
change reload keys for templated routes only — a missed reload becomes a
correct reload, never a spurious one. Diagnostics aggregation changes error
output shape (camel-cli prints structured causes).

Accepted residual risk: the configured-threshold-reaches-templated-steps
scenario is pinned at input-consumption depth only — `BuilderStep::Processor`
is opaque (`OpaqueProcessor(StreamCacheService)`), so no repo test can fail
by reverting the threshold threading inside `materialize_and_compile`. The
`stream_cache_config` helper unit test pins the shared seam both paths
consume; an execution-level pin (run a materialized route with a `Body::Stream`
sized between default and configured threshold) is the follow-up if the
processor ever exposes its config. Tracked as deferred work in bd rc-f2kp.
