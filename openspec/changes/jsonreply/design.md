# Design: jsonreply

## Approach

Add a module-private `json_error_reply(status, code, message)` helper beside
`pipeline_error_to_reply`. The helper serializes the same `serde_json::json!`
object, retains the existing `unwrap_or_else(|_| "{}".to_string())` fallback,
and returns the same `HttpReply` shape with an application/json header. The
four affected match arms keep their warning calls and pass their existing
values to the helper.

Tests will exercise the mapped `CamelError` variants through
`pipeline_error_to_reply`, asserting status, headers, and parsed JSON fields.
An empty message case protects string serialization, and the helper's output
shape is covered without asserting JSON object key order.

## Affected crates

- `camel-http`: private helper extraction and behavior-parity unit tests.

## Architecture boundaries

This is an inbound HTTP component-local refactor. It does not change Runtime,
DSL, Services, Languages, Functions, transport negotiation, or public
contracts. It preserves the existing HTTP finalizer boundary and its warning
logs, as required by ADR-0012's handler-contract logging rule.

## Alternatives considered

- Keep four copies: rejected because it preserves the identified maintenance
  defect.
- Move the helper to another crate or expose it publicly: rejected because the
  behavior belongs to the HTTP finalizer and no shared contract is needed.
- Move warning logs into the helper: rejected because it would change log
  context and handler-site behavior.
