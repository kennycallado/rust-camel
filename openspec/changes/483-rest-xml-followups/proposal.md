# Proposal: 483-rest-xml-followups

## Why

Mission 481 added REST `binding: xml`. Two pre-existing gaps now sit directly
on that path:

- **bd rc-fus2z** — a server-side response marshal failure
  (`CamelError::TypeConversionFailed`) is rendered as HTTP 400 for every
  binding. A server bug that produces an unencodable response is reported as a
  client error, so clients may retry a request that can never succeed.
- **bd rc-26wzq** — `XmlDataFormat::marshal` rejects `Body::Xml` (for example
  the output of a user transform step) and every `Body::Bytes`, so an XML REST
  handler cannot return an already-serialized XML document.

Both defects are reachable from a declared REST operation; neither is caused by
mission 481.

## What Changes

- The REST response marshal step becomes an internal, non-deserializable step
  (`ResponseMarshal`). Its marshal errors are wrapped as
  `CamelError::ProcessorErrorWithSource`, which the HTTP finalizer already maps
  to 500 with a generic body. Request-side unmarshal errors keep their
  `TypeConversionFailed` → 400 mapping. Declared `marshal:` route steps keep
  their current error semantics.
- `XmlDataFormat::marshal` accepts `Body::Xml` unchanged (already-serialized
  XML) and validates `Body::Bytes` as UTF-8 before passing the bytes through
  unchanged. Invalid UTF-8 fails closed with a `TypeConversionFailed` that
  names `Body::Bytes` and the byte offset. No content sniffing, no re-encoding,
  no well-formedness re-parse, no new crate.
- Specs and docs record the new normative behavior in `camel-http`,
  `rest-dsl`, and `data-formats`.

Explicitly excluded: multipart handling, content-type sniffing, changing the
request-side 400 mapping, XML security policy, and any change to `json`/`form`/
`raw` wire behavior (their step order and accepted media are unchanged).

## Acceptance criteria

- A response-side marshal failure returns HTTP 500 under `binding: json`,
  `form`, and `xml`; a malformed request still returns 400 `bad_request`
  (existing 481 suite stays green).
- A user step producing `Body::Xml` under `binding: xml` returns 201/200 with
  the exact XML bytes and the declared `produces` Content-Type.
- A user step producing invalid UTF-8 `Body::Bytes` under `binding: xml`
  returns 500, and `XmlDataFormat::marshal` returns a diagnostic naming
  `Body::Bytes` and UTF-8.
- `openspec validate --all --strict` passes; targeted
  `cargo test -p camel-dsl -p camel-processor -p camel-component-http`, fmt,
  clippy `-D warnings`, xtask lints, schema and rustdoc gates pass.

## Risk budget

Acceptable: a previously-400 response marshal failure becomes 500, and a route
`on_exceptions: kind: TypeConversionFailed` no longer catches REST response
marshal failures (they surface as `ProcessorErrorWithSource`). The same
identity change affects `do_try` catch-by-variant (`variant_name()` aliases the
variant to `ProcessorError`) and error trace/metric labels. All are the
intended correction and are recorded in the rest-dsl delta. Out of bounds:
changing request-side error mapping, shipping a new crate, or accepting
non-UTF-8 bytes into an XML response.
