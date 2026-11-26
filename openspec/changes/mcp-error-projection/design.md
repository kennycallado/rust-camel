# Design: mcp-error-projection

## Approach

One shared projection, two seams fixed at the MCP adapter boundary.

**Shared helper (camel-api).** New module `camel_api::error_projection` with:

- `pub fn business_envelope(code: &str, kind: &str) -> serde_json::Value` —
  exactly `{"error": <code>, "kind": <kind>}` (ADR-0097; `message`/`data` are
  log-only and never serialized).
- `pub const INTERNAL_ERROR_BODY_JSON: &str = r#"{"error":"Internal Server Error"}"#`
  — the generic non-business body, reusing camel-http's existing catch-all
  wording.

`camel-http::business_error_reply` delegates its body construction to
`business_envelope` (byte-identical: serde_json's default map sorts keys
alphabetically, `error` < `kind`, matching the current output). MCP cannot
depend on camel-http (component-on-component dep; `lint-component-deps`), so
the helper lives in the shared kernel next to `http_error_mapping`, which
already owns the ADR-0097 status half.

**Tool projection (consumer.rs tool bridge).** The bridge applies one
projection to a route failure however it arrives:

- `Err(e)` from `send_and_wait`, or `Ok(out)` with `out.error` still set
  (propagated failure — ADR-0097 §3 / bd `rc-7lkp2` family):
  - `CamelError::Business { code, kind, .. }` →
    `(business_envelope(&code, &kind), true)`. `isError: true` is already set
    by the flag path (`McpToolResult { is_error }` → `CallToolResult::error`);
    only the payload changes.
  - any other `CamelError` → the generic envelope, i.e.
    `serde_json::json!({ "error": "Internal Server Error" })`, plus
    `tracing::warn!(error = %e, tool = %name, "tool dispatch failed")` at the
    boundary — the detail lives in the log only. `CamelError`'s Display
    already excludes business `message`/`data` by contract, so the log line
    cannot leak them; it intentionally carries non-business detail (URLs,
    classes) that the client payload no longer holds.
- `Ok(out)` with no error and a cleared/handled error state stays a success:
  the body is serialized exactly as today. The no-internals rule governs
  projected failure bodies only — never successful content, protocol metadata
  (resource URIs, capabilities), or unrelated protocol errors
  (`resource_unavailable`, `invalid_params`).

**Resource projection (consumer.rs resource bridge + `body_to_resource`).**
The same two-way projection; failure content becomes the envelope JSON bytes
with mime `application/json` (replacing `e.to_string()` as `text/plain`). All
three failure arms change: the route-failure arms (resource bridge `Err(e)`
and `Ok`-with-carried-error) and the materialization-failure arm
(`body_to_resource`'s `Err`); the tool-side materialization arm
(`body_to_json`'s `Err`) gets the generic envelope + warn log too.

**Violation redaction (camel-json-schema + server.rs).** Probe conclusions
(jsonschema 0.60.2, verified against the vendored crate source; the
schema-path-segment idea from the first draft was corrected by the spec-bless
review):

- `ValidationError::to_string()` embeds the instance value (the leak).
- `ValidationErrorKind::keyword()` (`jsonschema/src/error.rs:342`) returns the
  JSON Schema keyword name (`"type"`, `"required"`, `falseSchema`, …) for
  every kind — this is the keyword source. Deriving the keyword from the last
  schema-path segment is WRONG: a nested boolean subschema's path ends in the
  property name (e.g. `/properties/value` for `{"properties": {"value":
  false}}`), not a keyword.
- The crate offers `error.masked()` (placeholder-substituted message) as a
  fallback; not needed under the strict contract below.

Additive API: `CompiledSchema::redacted_violations(&self, instance: &Value)
-> Vec<RedactedViolation>` with `pub struct RedactedViolation { pub keyword:
String, pub instance_path: String }`. Existing `Violation`/`violations()` are
untouched — the four current consumers (camel-processor, camel-validator,
camel-mock) keep their behavior.

**Instance-path redaction.** A JSON-pointer segment that names an object
member can itself be caller-controlled: `patternProperties` violations carry
the caller-invented member name inside `instance_path`
(`jsonschema/src/keywords/pattern_properties.rs`), so dropping validator
prose and capping the list do not close that leak. Redaction is therefore
**instance-aware**: `redacted_violations` walks the original instance
alongside the pointer segments — a segment passes through verbatim only when
the current instance node is an `Array` and the segment parses as a valid
array index, or when the decoded segment (`~1`→`/`, `~0`→`~`) is a property
name declared by the schema (the escaped spelling is preserved verbatim);
every other segment renders as `<prop>`, and once the walk leaves the known
instance shape (missing intermediate node, non-container node) the remaining
segments are masked unless declared. This defeats numeric-looking object
keys (`{"123456789": 5}` under `patternProperties {"^[0-9]+$"}` masks to
`/<prop>`; a genuine array index `/1` survives). The declared set is
collected once per call from the operator-authored schema `Value` stored on
`CompiledSchema` at construction (every `properties` object's keys, arrays
and objects traversed, depth-capped at 32 — small, trusted config per
ADR-0032). Note: jsonschema reports a `patternProperties` child failure with
the child keyword (`type`) at the member's pointer — the keyword comes from
`kind().keyword()`, the masked pointer from the walk above.

**Boundary wiring.** `validate_arguments` switches to the redacted accessor,
formats `"keyword at instance_path"`, and **caps the list at 25 entries**
(hard cap; excess dropped — the schema author sees enough violations to fix
authoring, logs stay bounded). No argument text reaches `ErrorData`; rmcp's
WARN of `ErrorData` (rmcp-3.9.0/src/service.rs:1625) therefore carries only
keyword + redacted path. The invalid-schema compile-error arm already names
only the tool and operator-authored schema text (trusted, ADR-0032) and stays
unchanged.

## Affected crates

- `camel-api`: new `error_projection` module (additive).
- `camel-http`: `business_error_reply` body delegates to the helper
  (byte-identical, no behavior change).
- `camel-json-schema`: additive `RedactedViolation` + `redacted_violations()`
  (with path redaction and keyword from `kind().keyword()`).
- `camel-component-mcp`: consumer projection (tools + resources, including
  carried-error arms), `validate_arguments` redaction + cap. The fix.

## Architecture boundaries

Runtime/kernel (`camel-api`) owns the envelope contract once; components
consume it. No DSL change, no service change. The MCP client producer path is
out of scope. Component lane (`cargo test -p camel-component-mcp`) covers the
adapter surface; hexagonal boundary test unaffected (no api→component edge
inversion).

## Alternatives considered

- **MCP depends on camel-http** — rejected: component-on-component dependency,
  fails `lint-component-deps`.
- **Duplicating the envelope inline in MCP** — rejected by the order: "do not
  invent a second one".
- **Extending `Violation` with `keyword`** — rejected: breaking struct change
  rippling into three consumer crates; additive accessor keeps the change
  inside two crates' behavior.
- **Using `error.masked()` messages** — rejected: still ships validator prose;
  the contract is keyword + redacted instance path only.
- **Keyword from schema-path last segment** — rejected after the spec-bless
  review: nested boolean subschemas end the path in a property name;
  `kind().keyword()` is authoritative.
- **Echoing instance paths verbatim** — rejected: `patternProperties` embeds
  caller-controlled member names; undeclared segments are redacted to
  `<prop>`.
- **Skipping the resource path or the carried-error arm** — rejected: both are
  the same leak; ADR-0097 §3 documents failures arriving inside `Ok(exchange)`.

Bd: `rc-gu99o`, `rc-3rob8`.
