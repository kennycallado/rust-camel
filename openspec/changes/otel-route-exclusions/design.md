# Design: otel-route-exclusions

## Approach

### Sampler chain (R2)

Current: `Sampler::ParentBased(LinkAwareSampler { inner: Sampler })`. New:

```
ParentBased(LinkAwareSampler { inner: RouteExcludeSampler { patterns, inner: Sampler } })
```

- `LinkAwareSampler` stays outermost inside `ParentBased`: a parentless span
  with links follows the first link's sampled flag before exclusion is
  consulted (T4). A linked `split-item` root therefore inherits its origin's
  decision and needs no name rule. T4 keeps the required unsampled-link case
  and adds a sampled-link counterpart, both with a `route_id` that matches the
  exclusion, so an exclusion-first implementation fails.
- `RouteExcludeSampler` handles only link-less roots. Parented decisions
  never reach it because `ParentBased` short-circuits first (T3). It scans
  creation attributes for `route_id` (route root spans set it at creation in
  `route_compiler.rs`), glob-matches case-sensitively against the full route
  id, and returns `Drop` on a match; otherwise it delegates to the inner
  sampler (T1/T2). A sampler with zero patterns delegates unconditionally
  (R5).
- `OtelSampler` is unchanged. Exclusion rides on `OtelConfig` as
  `exclude_routes: Vec<String>`. `to_sdk_sampler(&OtelConfig) ->
  Result<Sampler, CamelError>` compiles the patterns fallibly (no
  `expect`/`unwrap` in production), builds the `RouteExcludeSampler`, and
  wraps it as today. `init_tracer_provider` and `start` propagate the error.

### Config (R1, R5, T5)

`OtelCamelConfig` gains `#[serde(default)] pub exclude_routes: Vec<String>`
(the struct is `deny_unknown_fields`, so the key must be declared) and a
matching `Default` entry. `CamelConfig::validate()` compiles every pattern
with `glob::Pattern::new` and returns
`CamelError::Config("invalid [observability.otel] exclude_routes pattern
'<pattern>': <error>")`. Validation runs independently of `enabled`, so a
disabled OTel block still fails a bad pattern at load. `OtelConfig::validate`
applies the same check for direct Rust-API users. `context_ext.rs` passes
the list through to `OtelConfig`.

T5 loads a real TOML file with `tempfile::NamedTempFile` and
`CamelConfig::from_file`, not a hand-built struct. The loader maps
`validate()` errors to `config::ConfigError::Message`; the test asserts the
message names the bad pattern. A second test calls `validate()` directly to
pin the `CamelError::Config` variant.

`OtelConfig` is a `pub` struct without `#[non_exhaustive]`, so the field
addition touches: the field, `new()` default, a `with_exclude_routes`
builder, and the manual redacting `Debug` impl. Existing struct-literal test
sites use functional update syntax from `OtelConfig::new(...)`.

### Bridge layer (R4, T7)

In tracing-opentelemetry 0.34, `tracing_opentelemetry::layer()` returns
`OpenTelemetryLayer<S, NoopTracer>` and does not read the global provider.
Production never calls `.with_tracer`, so the bridge layer exports no
`tracing` spans. Engine spans (`pipeline_step`, `error_handler`, `function`,
and the `camel_tracer` `step` twin) are not orphan OTel roots. R4 is
therefore:

1. Correct the comment at `crates/services/camel-otel/src/service.rs:351`:
   the early `init_tracer_provider()` exists because the global tracer
   provider must exist before route spans are created (the engine mints
   spans through the global OTel tracer). Logger initialization is separate.
   The comment states that `tracing_opentelemetry::layer()`
   uses a no-op tracer by default and does not read the global provider.
2. Add T7 as a real route negative control. The test installs a real global
   `SdkTracerProvider` over `InMemorySpanExporter`, then runs a normal timer
   route with one exchange inside
   `tracing::subscriber::with_default(subscriber, || current_thread_runtime.block_on(...))`.
   The subscriber contains the production-equivalent
   `tracing_opentelemetry::layer()` (no `.with_tracer`) plus a `fmt` layer
   writing to a buffer. The subscriber stays installed across the
   current-thread runtime so actual route execution is observed. Assertions:
   exported span names are exactly the SDK route root and its SDK step spans;
   no span is named `pipeline_step`, `error_handler`, `function` or `step`;
   the `fmt` buffer contains the route log line.
3. Optional test-only control: a second synchronous test wires
   `tracing_opentelemetry::layer().with_tracer(<local tracer>)` and shows the
   same named spans WOULD export. This is test-only evidence for why
   production must not wire a real tracer; it does not replace T7 and adds no
   production API.

No `bridge_span_filter`, no `is_engine_span_target`, no `ENGINE_SPAN_TARGETS`
and no production `.with_tracer` are introduced.

### Metrics (R3)

No code change. `record_step_metrics` in `tracer.rs` runs on the collector
path, independent of sampling; a dropped root yields non-recording spans but
the collector calls are unaffected. T6 verifies exchange-count and duration
metrics for both an excluded and a non-excluded route.

## Affected crates

- `camel-otel`: `RouteExcludeSampler` + `LinkAwareSampler` wiring in
  `service.rs`; `OtelConfig.exclude_routes`, validation and builder in
  `config.rs`; tests T1-T4 in `sampler_tests.rs`; T6/T7 in a new
  `route_exclusion_tests.rs`. Regular dependency `glob` (runtime need).
  Dev-dependencies gain `camel-component-timer`, `camel-processor` and
  `tracing-opentelemetry`.
- `camel-config`: `OtelCamelConfig.exclude_routes` + `Default` + validation
  in `config.rs`; `context_ext.rs` plumbing; T5 in
  `config_tests/config_validation_tests.rs`. `glob` is already a regular
  dependency.
- Docs: `docs/src/services/tracing.md`,
  `docs/src/configuration/schema.md`.

## Architecture boundaries

Services-context change only (observability). No DSL/AST/schema-of-routes
change. No Runtime behaviour change beyond sampling decisions at span
creation. The config contract is extended additively with an empty default.

## Decisions

- **D1 CHANGELOG**: the repo has no `CHANGELOG.md`; it was removed
  deliberately (`8b54d1e2 "chore(root) clean"`). Release notes are generated
  from Conventional Commits by `cargo xtask changelog`. Recreating the file
  would undo a deliberate cleanup, so R6's "CHANGELOG entry under Unreleased"
  is met through generated notes. The landing commits use
  `feat(otel): add exclude_routes route span exclusion` and
  `docs(otel): correct bridge layer provider comment`. This deviation from
  the mission's literal wording is recorded in the park report.
- **D2 Test placement**: T1-T4 are unit tests in `sampler_tests.rs`. T5 is a
  camel-config unit test. T6 and T7 are integration-shaped unit tests inside
  camel-otel (`#[cfg(test)]`, `serial_test`) because `InMemorySpanExporter`
  injection and the sampler constructors are crate-private and camel-config's
  `otel` feature is not default. T6 boots a real `CamelContext` with two
  timer routes. T7 runs a real route under a scoped subscriber and a real
  global SDK provider. Both use the unkeyed `serial_test` attribute, so they
  share the default serial key and cannot interleave their global provider
  writes.
- **D3 Golden risk**: camel-config already depends on `glob`, so its closure
  is unchanged. camel-otel gains `glob`, which may change the
  `feature_profiles` golden. Regenerate ONLY if the closure genuinely
  changed, with the reason recorded.
- **D4 Docs completeness**: `tracing.md` documents glob semantics
  (case-sensitive, full-match on the route id) and the parent-based sampling
  contract. A matching route that starts a new trace exports no root or child
  spans. A sampled inbound parent or sampled link still wins over exclusion.
  Child routes inherit the parent sampling decision. Metrics remain unchanged.
  Invalid patterns fail config load even when `enabled = false`.
- **D5 rc-ue6bv**: closed as not reproduced. The bridge layer is a no-op by
  default; no production orphan-root mechanism exists from this layer. R4
  delivers only the comment correction and T7 negative control.
