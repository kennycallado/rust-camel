# Proposal: mcp-error-projection

## Why

Two verified p2 defects share one seam — the MCP server adapter's failure
surface (`rc-gu99o`, `rc-3rob8`):

1. **Failures bypass the ADR-0097 envelope.** The tool bridge serializes any
   route failure as `{"error": e.to_string()}`
   (`camel-component-mcp/src/consumer.rs:527`), and the resource bridge writes
   the same string as `text/plain` content (`consumer.rs:583`, `consumer.rs:664`).
   For a business failure the client sees only `business error <code> (<kind>)`
   — the structured envelope is lost. For a non-business failure the raw
   `CamelError` text reaches the client: reqwest error text carries the full
   target URL and error class (via `camel-http/src/ssrf.rs:373`), violating
   ADR-0097:145-148 (`message`/`data` are log-only) and leaking URLs and Rust
   type names in client-visible payloads.
2. **Argument values reach WARN logs.** `validate_arguments`
   (`camel-component-mcp/src/adapter/server.rs:523-534`) puts jsonschema's
   `error.to_string()` — which embeds the offending instance value — into
   `data.violations`. rmcp logs the whole `ErrorData` at WARN on handler
   failure (`rmcp-3.9.0/src/service.rs:1625`), so up to 1 MiB of
   adversary-supplied argument text lands in operator logs per rejection,
   with no redaction hook in the SDK.

## What Changes

- **Shared projection helper in `camel-api`**: one function producing the
  ADR-0097 business envelope JSON (`{"error": <code>, "kind": <kind>}`) and one
  constant for the generic non-business body. `camel-http`'s
  `business_error_reply` delegates to it (byte-identical body); the MCP server
  uses it. No second projection is invented.
- **MCP consumer projection** (tools AND resources): business failure →
  `isError: true` + envelope exactly; non-business failure → generic body with
  the detail logged at the adapter boundary (`tracing::warn!`). Applies to the
  tool bridge, the resource bridge, and the body-materialization arms.
- **Violation redaction**: `camel-json-schema` gains an additive
  `redacted_violations()` accessor (keyword via jsonschema's
  `ValidationErrorKind::keyword()`; instance-aware redacted instance path;
  existing `violations()` untouched). `validate_arguments`
  uses it, caps the violation list at 25, and no argument text reaches
  `ErrorData`.

Affected crates: `camel-api`, `camel-http` (mechanical delegate, no behavior
change), `camel-json-schema` (additive), `camel-component-mcp` (the fix).
Spec delta: `mcp-component` — two ADDED requirements.

Explicitly excluded: HTTP status mapping in MCP (none exists), rmcp SDK
changes, producer (client) error paths, changes to `CamelError` Display.

## Acceptance criteria

- `business_failure_uses_adr0097_envelope`: `throw:` in a tool route →
  `isError: true`, content exactly `{"error":<code>,"kind":<kind>}`, no
  `message`/`data`.
- `technical_failure_body_is_generic`: HTTP failure with a URL in reqwest text →
  payload has no `http://`, no `ssrf`, no type name; the log line carries the
  detail.
- `resource_failure_uses_same_projection`: resource-read failure uses the same
  two projections.
- `violations_exclude_instance_values`: 1 MiB argument violating the schema →
  value never appears in `ErrorData`; violation list capped at 25.
- All gates green (see tasks.md); `cargo test -p camel-component-mcp` lane
  passes.

## Risk budget

- Low risk: camel-http delegation is a byte-identical refactor guarded by its
  existing suite.
- Additive camel-json-schema API: no existing consumer changes.
- Out of bounds: redefining `CamelError` Display, touching rmcp, altering HTTP
  status semantics (ADR-0096/0097 status rules).

Bd: `rc-gu99o` (envelope bypass), `rc-3rob8` (violation leak).
