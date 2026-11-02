# Proposal: http-header-case-semantics

## Why

`Message.headers` is a plain `HashMap<String, Value>`. `set_header` stores the
caller's spelling verbatim, `header` matches exactly, and `header_ic` scans
nondeterministically when two spellings coexist. The HTTP consumer then reads
`header("Content-Type")` exactly when building the reply. Consequences
reported in bd rc-qtpq1:

- `set_header: content-type` in a route does not control the HTTP reply;
  `set_header: Content-Type` does. A lowercase key is dropped, not shadowed.
- The inbound request `Content-Type` stays on the exchange, so a route that
  sets nothing still echoes the request media type on the reply.
- `header_ic` is unsafe as an interim fix: `HashMap` iteration makes the
  winner random when duplicate spellings exist.

## What Changes

- **Header-map contract (API-003).** Keep the public `HashMap` field (no
  breaking type migration). Define exact vs case-insensitive operations:
  `header` stays exact-case; `header_ic` becomes deterministic (exact match
  first, then ASCII-case-insensitive match); `set_header` replaces every
  case-insensitive duplicate before inserting the caller's spelling;
  `remove_header` removes every case-insensitive match; `set_header_normalized`
  deduplicates too. Route steps (`SetHeader`, `DynamicSetHeader`,
  `DynamicSetHeaderIfAbsent`, `RemoveHeader`) use these methods instead of
  direct map writes.
- **HTTP reply content type.** The reply finalizer reads the route's
  content type case-insensitively and ignores an inherited inbound request
  content type that the route did not change. `select_response_headers`
  continues to drop content-type keys case-insensitively and appends exactly
  one `Content-Type`.
- **Spec/docs.** New `message-header-semantics` capability; amend the
  `http-emission-correctness` consumer-response requirement; document the
  boundary in the HTTP and API crate `CONTEXT.md`; remove `TODO(API-003)`.

Explicitly excluded: replacing the map type, inbound header title-casing
policy, header-value redaction, broad reply-header redesign.

## Acceptance criteria

- Lower/upper/mixed-case `set_header`/`header_ic`/`remove_header` agree.
- A route `set_header("content-type", ...)` controls the HTTP response.
- An untouched inbound request `Content-Type` is not echoed.
- Duplicate-case insertion yields a deterministic `header_ic` result.
- Header iteration/serialization preserves the caller's spelling.
- Existing HTTP and e2e suites stay green.

## Risk budget

Behavioral widening is limited to case-insensitive lookup/mutation helpers and
HTTP reply precedence; exact-case `header` and the map type stay unchanged.
Value equality with the recorded request content type is the normative
provenance boundary (a route setting exactly the request value is treated as
inherited); this is specified, not a hidden side effect.
