# Proposal: 481-rest-xml-binding

## Why

The REST DSL offers three binding modes: `json` (default), `raw` (verbatim
passthrough), and `form` (landed in change 475). Any XML body — `application/xml`,
`text/xml`, or a `+xml` structured-suffix type such as `application/atom+xml` —
still forces route authors into `binding: raw` plus hand-written parse and
synthesis steps, and gives up declared-schema validation entirely.

The framework already ships a bounded XML data format (`XmlDataFormat` in
`camel-processor`, registered as `xml`, with a nesting-depth cap). XML binding is
the last slice of umbrella `rc-yaqrf` with a sane effort/value ratio: multipart
needs a streaming-body redesign and stays out. bd: `rc-gk7jt` (P2), umbrella
`rc-yaqrf`.

## What Changes

- New REST DSL binding mode `binding: xml` (`RouteDslRestBinding::Xml`).
  `consumes` and `produces` must be XML media: `application/xml`, `text/xml`, or
  a `type/*+xml` subtype, optionally followed by exactly one `charset=utf-8`
  parameter. Anything else fails route load with a `RouteError` naming the
  operation, the field, the offending value, and the accepted media family.
- Lowering injects `Unmarshal { unmarshal: "xml" }` for body-bearing verbs and
  `Marshal { marshal: "xml" }`; negotiation, user-step ordering, declared
  `produces` Content-Type, and default-status injection stay identical to `json`
  and `form`.
- `request_schema` and `response.schema` stay allowed. The request body is
  unmarshalled XML → JSON first, then schema-validated, so schemas describe the
  JSON projection of the XML document. Response schemas remain OpenAPI-declared
  metadata, as they already are for `json`.
- `XmlDataFormat::marshal` gains `Body::Empty` passthrough so an XML REST
  operation that returns no body (for example `success_status: 204`) does not
  fail at the response marshal step. This matches the existing `json` contract;
  no other `XmlDataFormat` behavior changes.
- Tests (media matrix, lowering sequence, executed request/response pipeline,
  malformed-XML typed error on the HTTP path, json/raw/form regressions), docs
  (REST DSL binding table, data-format page, schema ordering), and a `rest-dsl`
  spec delta (binding closed set becomes `json|raw|form|xml`; ADDED XML
  requirements).

Excluded: multipart, a new crate, XML-specific config knobs, content-type
sniffing, changes to `json`/`raw`/`form` behavior, and XML security-policy
changes (`xml_convert` already refuses external entities and caps depth).

Affected crates: `camel-dsl` (binding, media helper, lowering, OpenAPI parity
tests), `camel-processor` (`XmlDataFormat` empty-body parity + tests). Docs under
`docs/src`.

## Acceptance criteria

- `binding: xml` accepts `application/xml`, `text/xml`, and `+xml` subtypes
  (with at most one `charset=utf-8` parameter) for both `consumes` and
  `produces`; every other value fails closed with an actionable `RouteError`.
- Unknown binding strings still fail at deserialize time (closed enum).
- Lowering yields `unmarshal: xml` for POST/PUT/PATCH and `marshal: xml` for
  every verb, with user steps, negotiation, Content-Type, and default status in
  the same positions as `json`/`form`.
- `json`, `raw`, and `form` lowering and error strings are byte-identical to
  `main`; their existing tests pass unchanged.
- An XML request reaches user steps as `Body::Json`; a JSON response leaves the
  pipeline as XML text with the declared `produces` Content-Type. Malformed XML
  surfaces from the pipeline as `CamelError::TypeConversionFailed`; the existing
  HTTP mapping (`type_conversion_failed_maps_to_400` in
  `crates/components/camel-http/src/lib_tests.rs`) covers the 400 reply and is
  not re-tested here.
- `cargo test -p camel-dsl -p camel-processor` green; fmt, clippy
  (`-D warnings`, all targets for touched crates), all xtask lints, schema
  check, rustdoc `-D warnings`, `mdbook test`, and `openspec validate --strict`
  green.

## Risk budget

Acceptable: one new enum variant, one media predicate, one lowering arm, one
match arm in the format selector, one additive `Body::Empty` arm in
`XmlDataFormat::marshal` with a test, docs, spec delta. Out of bounds: touching
`json`/`raw`/`form` semantics or error text, new crates, new configuration
surface, stream consumption changes, content-type sniffing, multipart.
