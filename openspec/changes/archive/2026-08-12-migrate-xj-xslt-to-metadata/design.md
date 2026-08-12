# Design: migrate-xj-xslt-to-metadata

## Approach

Add a metadata-only descriptor struct to each component following the
established `skip_impl` pattern used by ~20 other components (jms, kafka,
redis, cxf, timer, etc.). The descriptor is a phantom struct: fields are
prefixed `_` and never read at runtime — `skip_impl` generates `metadata()` +
`uri_options()` only, no parser. The runtime `from_uri` parsers in `config.rs`
remain the source of truth for parsing and stay unchanged.

The critical wiring (e_opus blocker B2): a metadata-only struct publishes
nothing by itself. Metadata reaches the catalog only when `Component::metadata()`
is overridden. Both `XjComponent` and `XsltComponent` currently inherit the
`minimal(scheme)` default. This change adds the override on each, returning the
descriptor's `metadata()`.

### Field declarations (incorporating e_opus fixes)

Every field carries an explicit `name = "camelCaseKey"` matching the runtime
parser's query keys. Optional query params are `Option<T>` or carry
`default = "..."` to avoid the macro's required-flag inference
(`required = !is_option && default.is_none()`). The URI path component
(`stylesheet_uri`) is NOT declared as a `uri_param` — it is never a query key,
and declaring it would mark it required, producing a false positive on every
route.

**xj descriptor fields:**
- `direction: String` (required — correct, runtime rejects absence)
- `params: Vec<(String, String)>` with `pattern = "param."` (open namespace)
- `max_payload_bytes: Option<usize>` (`maxPayloadBytes`)
- `retry_count: u32` with `default = "3"` (`retryCount`)
- `retry_delay_ms: u64` with `default = "500"` (`retryDelayMs`)

**xslt descriptor fields:**
- `params: Vec<(String, String)>` with `pattern = "param."` (open namespace)
- `output_method: Option<String>` (`output`)
- `transformer_cache_size: Option<usize>` (`transformerCacheSize`)
- `fail_on_null_body: bool` with `default = "false"` (`failOnNullBody`)
- `max_payload_bytes: Option<usize>` (`maxPayloadBytes`)

The `pattern = "param."` separator yields option name `"param"` (trailing `.`
stripped per macro derivation). Parity tests assert this exact name and also
assert that numeric field types (`usize`, `u32`, `u64`) derive `OptionKind::Int`
(not `String`), preventing silent kind-inference regressions that would cause
false `KindMismatch` diagnostics on valid routes.

## Affected crates

- `crates/components/camel-xj`: new `src/metadata.rs`; `src/lib.rs` gains
  `mod metadata;`; `src/component.rs` gains `fn metadata(&self)` override;
  new parity test module.
- `crates/components/camel-xslt`: same shape — new `src/metadata.rs`;
  `src/lib.rs` + `src/component.rs` wiring; parity test module.
- `crates/camel-cli` (test only): `lint_corpus` audit of xj/xslt example
  routes; the catalog builtin at `src/lib.rs:106-107` already registers both
  components, so no registration change is needed.
- `schemas/component-metadata.json`: regenerated snapshot (additive — new
  options for xj/xslt).
- `openspec/specs/endpoint-metadata-derivation/spec.md`: MODIFIED scenario +
  new scenario (delta applied at archive time).

## Architecture boundaries

This change stays entirely within the **Components** layer for implementation
and touches the **metadata/catalog** surface (which spans Components →
camel-api contract → camel-lint consumer → camel-cli catalog builtin →
schema-gen). It does NOT touch:

- **Runtime** (no change to endpoint creation, bridge, or tokio affinity).
- **DSL** (no change to route parsing or compile-time validation).
- **Services** (no change to processor/pipeline execution).
- **Languages** (no change to Simple language or data formats).

The data/control-plane boundary is respected: metadata is control-plane
information; the runtime parsers (data-plane) are untouched.

Relevant ADR: ADR-0041 (Component Metadata Capabilities Schema) — this change
is the second consumer of the open-namespace amendment added by Change 1.

## Alternatives considered

1. **Annotate the existing `XjEndpointConfig` / `XsltEndpointConfig` directly
   (no separate descriptor).** Rejected: the runtime structs use hand-rolled
   `from_uri` with custom validation (enum parsing, explicit rejection of
   unsupported params). Replacing with macro-generated parsing would require
   re-implementing that logic in macro attributes, which the macro does not
   support. The `skip_impl` descriptor is the only shape that publishes
   metadata without replacing the parser.

2. **Split into two changes (one per component).** Rejected (e_opus Q5): both
   share the identical migration shape and the same canonical-spec disposition
   update. One change with two parallel tasks is cleaner and avoids two delta
   specs touching the same requirement.

3. **Add enum-value validation for `direction`.** Rejected (e_opus Q2/B2): the
   metadata model has no value-set constraint. Inventing one would require
   changes to the macro, `OptionKind`, and the resolver — a separate change.
   Plain `String` + desc listing valid values is the honest ceiling.
