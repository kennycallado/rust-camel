# Design: xslt-header-driven-params

## Approach

Follow the mission's default: header-driven `paramFrom.<name>=<header-key>` URI options.
The data path stays inside camel-xslt; the bridge protocol is untouched (the transform
request already carries `parameters: map<string,string>`).

**Decided semantics (single source of truth; docs restate this):**

1. **Resolution timing.** Per exchange, inside `XsltProducer::call`, before
   `transform_with_retry`. Each call builds a fresh `Vec<(String, String)>` — static
   params cloned, then dynamic entries appended (the client collects into a
   `HashMap`, so later entries win → dynamic overrides static). No shared mutable
   state, so params cannot leak across exchanges by construction.
2. **Precedence.** Header present for `paramFrom.<name>` → its converted value is sent
   for `<name>`, overriding a static `param.<name>` of the same name. Header absent (or
   `Value::Null`) → no dynamic entry for `<name>`; a same-name static `param.<name>`
   flows through unchanged. Neither → parameter omitted entirely. Omission never sends
   an empty string; an explicit `Value::String("")` header is user data and is
   forwarded verbatim.
3. **Value rules.** `String` → verbatim. `Number(n)` → `n.to_string()` (serde_json
   Display, canonical). `Bool(b)` → `"true"`/`"false"`. `Array`/`Object` → fail closed:
   `CamelError::ProcessorError` naming the param name, header key, and value type via
   `camel_api::value::value_type_name` — never the value content (payload-free, matches
   the redaction posture). Rationale: no canonical Value-to-string rule exists in
   camel-api (checked `value.rs`: only `cmp_values`, `value_type_name`), so the
   mission's fail-closed branch applies.
4. **Parsing.** In `XsltEndpointConfig::from_uri`, a `paramFrom.` prefix arm mirrors the
   `param.` arm: reject empty name (`paramFrom.=h`) and empty header key
   (`paramFrom.x=`) with `EndpointCreationFailed`. `param.*` handling unchanged.
   Disjoint prefixes cannot collide.
5. **Plumbing.** `XsltEndpointConfig` gains `param_from: Vec<(String, String)>`
   (param name → header key). `XsltEndpoint` and `XsltProducer` gain the same field;
   `component.rs` passes it through. Resolution helper is a pure function
   (`resolve_params(statics, param_from, headers) -> Result<Vec<(String,String)>, CamelError>`)
   — unit-testable without a bridge.
6. **Tests.** Use the existing in-process seam `XsltBridgeClient::with_backend` with a
   recording fake `XsltTransformBackend` that captures `parameters` per transform and
   echoes them into the result bytes. No JVM, no sleeps. Cover: static-only unchanged;
   dynamic value reaches stylesheet; missing header omits; null omits; precedence
   (present header overrides static; absent header falls back to static); number/bool
   conversion; array/object fail-closed error names param without content; two
   exchanges with different header values carry isolated params.
7. **Metadata.** `metadata.rs` registers a `paramFrom` prefix-pattern option
   (`UriOptionMatch::Prefix`), mirroring `param`; the metadata names test gains
   `paramFrom`.
8. **Docs.** `docs/src/components/xml-transform.md` xslt section: syntax line, table
   rows for `paramFrom.<name>`, examples for static, dynamic, precedence, missing
   header. Crate-level docs on `XsltEndpointConfig` updated. Spec check (done at
   proposal time): no requirement in `openspec/specs` describes the xslt param surface
   → no spec change; docs carry the contract; `skip_specs: true`.

Deferred: expression-driven `paramExpr.<name>` (mission-sanctioned deferral, bd ticket
at park time).

## Affected crates

- `camel-xslt`: `config.rs` (parse), `endpoint.rs`/`producer.rs`/`component.rs`
  (plumbing), `metadata.rs` (option registration), tests.
- `docs`: `docs/src/components/xml-transform.md`.

## Architecture boundaries

Components boundary only. camel-xslt consumes `camel_api::{Value, value::value_type_name}`
and the existing `camel_component_api::{Exchange, CamelError}` contracts; no camel-api or
camel-core changes; no bridge/proto changes; no DSL/Runtime/Services/Languages/Functions
surface touched.

## Phases

Single-phase change: config + producer + tests + docs form one coherent slice in one
crate. No `## Phase N` grouping.
