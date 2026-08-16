# Design: add-endpoint-parameters

## Approach

Introduce a typed seam for endpoint URIs at the authoring boundary while keeping the runtime string-end-to-end. A new `EndpointUri` value type in camel-api owns the three concerns that previously lived in ad-hoc string surgery: fail-closed merging of `uri` + `parameters:`, deterministic canonical rendering, and secret redaction. camel-dsl lowers both authoring surfaces through it at the compile boundary; camel-lint extracts `parameters:` entries as spanned options so existing rules cover the map. Runtime, `CanonicalStepSpec`, journal, and every component `from_uri` parser stay byte-identical.

Per the amended e_opus verdict (2026-08-15): this is option "A+" — the blessed rc-6vni design with the merge logic housed in a reusable type. "B" (EndpointUri inside CanonicalStepSpec) is rejected: it fails the ADR-0016 six-criteria test and detonates the ts_rs/schemars/journal surface for zero runtime gain. "C" (per-component typestate codegen) is rejected: camel-lsp already delivers that DX over `uri_options()`.

## Affected crates

- **camel-api**: new module `endpoint_uri.rs` — `EndpointUri` struct, `try_from_uri_and_params`, `to_canonical_string`, `to_redacted_string` + redacting `Debug`. No changes to existing types.
- **camel-dsl**: `route_ast.rs` structs (`ToStep`, `WireTapStep`, `EnrichStep`, `PollEnrichStep` — the latter two via the shared `EnrichBody`/`EnrichConfig` — and the route-level `from` surface) gain `parameters: BTreeMap<String,String>` held as the raw pair; the AST→model lowering in `yaml.rs` (shared by the YAML and JSON authoring paths) merges via `EndpointUri::to_canonical_string()` so `model.rs` types receive the canonical merged `uri: String`; `compile.rs` consumes canonical model URIs unchanged; `#[non_exhaustive]` on the endpoint AST structs; `route-schema.json` regenerated to admit the `parameters` key.
- **camel-builder**: RouteBuilder gains optional `.parameters(map)` — consuming-self signature, attaches to the most recent endpoint slot (from before steps; to/wire_tap/enrich/poll_enrich after), misuse and duplicate-key conflicts error at `build()` through the established builder error policy.
- **camel-lint**: `route_view.rs` LintRoute construction appends `parameters:` entries as `LintOption`s with spans into the source map; embedded schema copy (`crates/camel-lint/schema/route-schema.json`) re-synced byte-equal with camel-dsl's regenerated schema; corpus baseline (RON) updated with a secret-in-parameters fixture.

## Architecture boundaries

- **Runtime untouched**: `CanonicalStepSpec::To/WireTap`, `RouteDefinition.from_uri`, `BuilderStep` all keep `uri: String`. The collapse happens at the compile boundary in camel-dsl — below that line the persisted bytes are exactly what the pre-change code would produce (`source_hash` stable by construction).
- **ADR-0051 compliance**: `EndpointUri` is classified `redacting-wrapper` — no `Serialize` derive (never crosses a persistence boundary), manual redacting `Debug`, two explicit renderings (canonical for runtime, redacted for logs/diagnostics).
- **camel-lint charter intact**: no new dependency on camel-dsl/camel-core; spans remain byte-exact into the original source; changes are LintRoute construction only, zero rule-semantics changes.
- **camel-lsp untouched**: delegates to the lint engine, so it inherits parameters coverage with zero code changes (its spec's dependency constraint is unaffected).

## Phases

### Phase 1: EndpointUri value type in camel-api
- **Goal:** the typed seam exists, tested in isolation.
- **Dependencies:** none (camel-api is the bottom of the graph).
- **Externally-visible types/interfaces:** `EndpointUri`, the `EndpointUriError` type plus the `CamelError::EndpointUri` variant, re-export in camel-api prelude/lib.
- **Deliverable:** `crates/camel-api/src/endpoint_uri.rs` + unit tests.
- **Exit-criteria:** fail-closed duplicate-key test; determinism golden-string test; redaction test over a stub catalog; round-trip `to_canonical_string` → representative `from_uri` re-parse.

### Phase 2: parameters: surface in camel-dsl
- **Goal:** the authoring AST accepts `parameters:` on all endpoint surfaces and lowers through EndpointUri.
- **Dependencies:** Phase 1.
- **Externally-visible types/interfaces:** `parameters` field on the endpoint AST structs, `.parameters(map)` on RouteBuilder endpoints (camel-builder).
- **Deliverable:** updated route_ast.rs/yaml.rs lowering/camel-builder + regenerated `route-schema.json` + tests; any in-tree struct-literal constructions of the endpoint structs updated for `#[non_exhaustive]`.
- **Exit-criteria:** equivalent-canonical-string test; duplicate-key compile-error test (both YAML and JSON); poll_enrich parity test; existing DSL test suite green; `cargo xtask schema --check` green (schema regenerated AND camel-lint's embedded copy re-synced byte-equal); `#[non_exhaustive]` added.

### Phase 3: camel-lint parameters extraction
- **Goal:** lint rules cover `parameters:` entries; corpus updated.
- **Dependencies:** Phase 2 (surface must exist to lint).
- **Externally-visible types/interfaces:** none (internal LintRoute construction).
- **Deliverable:** route_view.rs extraction + corpus fixture + baseline regen.
- **Exit-criteria:** R-SECRET fires with span inside `parameters:` map; unknown-param/missing-required/deprecated also fire from map entries; zero-false-positives corpus gate green; embedded schema byte-equal gate green.

## Alternatives considered

- **A (free synthesize_uri helper in camel-dsl)**: blessed original; rejected in favor of A+ because the type is near-zero marginal cost, centralizes redaction, and converts every future extension (OpenAPI param binding, REST DSL) from "invent the type" to "extend it".
- **B (EndpointUri in CanonicalStepSpec)**: rejected — ADR-0016 six-criteria fail; breaks TS bindings and journal compat for no runtime gain.
- **C (per-component typestate builders)**: rejected — build-ahead; camel-lsp already delivers authoring DX over `uri_options()`.
