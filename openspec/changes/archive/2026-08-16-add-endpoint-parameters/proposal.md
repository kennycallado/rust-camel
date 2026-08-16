# Proposal: add-endpoint-parameters

## Why

Endpoint URIs in YAML/JSON routes are single strings (`timer:tick?period=1000&password=x`). This is error-prone: typos in query keys are invisible until runtime, secrets sit in plain query strings, and long URIs are unreadable. Apache Camel's YAML DSL solves this with a separate `parameters:` map. bd issue: rc-6vni (design verdict e_opus 2026-08-08, amended 2026-08-15).

Two recent landings raise the stakes: camel-lint (rc-5tm3, 08-10) and camel-lsp (08-11) both validate/query URI options via `uri_options()` metadata — but only from query strings. If `parameters:` landed without lint support, secrets moved into the map (as the secret rule itself recommends) would become invisible to the secret checker: a self-contradictory regression.

## What Changes

- **camel-api**: new `EndpointUri { scheme, path, params: BTreeMap<String,String> }` value type — `#[non_exhaustive]`, ADR-0051 `redacting-wrapper` (redacting `Debug` masking all param values, no `Serialize`), fail-closed `try_from_uri_and_params`, deterministic `to_canonical_string()` (existing query pairs preserved byte-for-byte, params appended sorted with pinned percent-encoding), fail-safe catalog-aware `to_redacted_string()`.
- **camel-dsl**: `parameters: BTreeMap<String,String>` on all endpoint surfaces (from, to, wire_tap, enrich, poll_enrich) in the route_ast.rs authoring AST (`ToStep`/`WireTapStep`/`EnrichStep`/`PollEnrichStep` via the shared `EnrichBody`/`EnrichConfig` hold the raw uri + parameters pair); the AST→model lowering in `yaml.rs` (shared by the YAML and JSON paths) merges via `EndpointUri::to_canonical_string()` so model.rs types (`DeclarativeRoute.from`/`*StepDef`) receive the canonical merged `uri: String`, unchanged in shape, and `compile.rs` consumes canonical URIs only; `#[non_exhaustive]` on the endpoint AST structs; `route-schema.json` regenerated to admit the `parameters` key.
- **camel-builder**: optional `.parameters(map)` on RouteBuilder, attaching to the most recent endpoint slot, errors surfaced at `build()` via the established builder error policy.
- **camel-lint**: route_view extracts `parameters:` map entries as span-carrying `LintOption`s so existing rules (unknown-param, missing-required, deprecated, secret, alias) cover the map with no rule changes; the embedded schema copy is re-synced byte-equal with camel-dsl's regenerated `route-schema.json`; corpus baseline gains a secret-in-parameters fixture. camel-lsp inherits with zero code changes.

**Excluded**: CanonicalStepSpec / runtime / journal format (stay `uri: String` — string end-to-end by ADR-0011/0016); parameters-block completion/hover in the lint engine (fast-follow bd rc-zn63); per-component typed builders (rejected, verdict §4).

## Acceptance criteria

- Route with `to: kafka:orders` + `parameters: {brokers: x}` compiles to the same canonical string as the equivalent query-string URI.
- A key present in both query string and `parameters:` produces a compile error (fail-closed), not a silent merge.
- Canonical string output is byte-deterministic (BTreeMap sorted, pinned percent-encoding) — stable under hot-reload `source_hash`.
- `EndpointUri` never exposes secret values through `Debug` or `Serialize`; `to_redacted_string` masks values whose `UriOption.secret` flag is set.
- Secret placed in `parameters:` triggers the R-SECRET lint diagnostic with a span pointing into the map entry; corpus baseline updated accordingly.
- camel-lsp requires no code change and its spec's no-core/no-dsl dependency constraint is untouched.

## Risk budget

Acceptable: additive DSL surface, compile-time-only behavior changes for previously-invalid input, and a bounded Rust source-compatibility impact on the endpoint authoring structs (`#[non_exhaustive]` plus new fields break out-of-crate struct-literal construction; in-tree these structs are populated by deserialization, so the impact is confined to tests/examples that construct them literally — updated in this change). Out of bounds: any change to `CanonicalStepSpec` serialization, journal format, component `from_uri` parsers, or camel-lint's runtime-free charter (no camel-dsl dependency, spans stay byte-exact, embedded schema stays byte-equal with camel-dsl's).
