# Design: 481-rest-xml-binding

## Context

REST lowering (`crates/camel-dsl/src/rest.rs::lower_operation`) branches on
`RouteDslRestBinding` (`crates/camel-dsl/src/route_ast.rs`). The XML data format
already exists: `XmlDataFormat` in `crates/camel-processor/src/data_format/xml.rs`,
name `"xml"`, registered in `builtin_data_format_with_config`, with
`XmlConfig { max_depth }` defaulting to `camel_api::xml_convert::DEFAULT_MAX_XML_DEPTH`
(100). `xml_convert` rejects `DOCTYPE` in `validate_xml`, never resolves external
entities, and caps nesting depth. ADR-0073 (strict negotiation), ADR-0030
(exchange-aware data formats), ADR-0032 (fail-closed parsing), ADR-0091 (style).
Data plane only: no component or control-plane change.

## Decisions

**D1 — Media predicate.** New `is_xml_media_type` in `crates/camel-dsl/src/media.rs`,
beside `is_form_media_type`, same shape and same parameter discipline. Contract:

- Base = `split_media_base` (outer trim, parameters dropped). Split once on `/`;
  both parts must satisfy `valid_header_token`.
- Subtype, ASCII-lowercased, must equal `xml` or end with `+xml` with a non-empty
  prefix (`application/+xml` is rejected). The type token is not restricted, so
  `image/svg+xml` and `text/foo+xml` pass — mirroring `is_json_media_type`.
- Parameters: at most one, and it must be `charset=utf-8` after trim,
  case-insensitive. This follows the `form` precedent because `XmlDataFormat`
  decodes `Body::Bytes` as UTF-8 only: accepting `charset=iso-8859-1` would hide a
  decode failure. Quoted parameter values are not part of the media subset and are
  rejected (`application/xml; charset="utf-8"`), as are empty parameters
  (`application/xml;`) and multiple parameters.
- Accepted: `application/xml`, `text/xml`, `application/atom+xml`,
  `image/svg+xml`, `application/xml; charset=utf-8`, `APPLICATION/XML`.
- Rejected: `application/json`, `text/plain`, `application/xmlx`, bare `xml`,
  `application/+xml`, non-UTF-8 or malformed parameters.

**D2 — Lowering.** Add the `Xml` arm to the validation `match` (checking both
`consumes` and `produces`) and to the data-format selector
(`Xml => "xml"`). The injected sequence is exactly the `form`/`json` sequence with
`"xml"` in place of the format name: negotiation, `Unmarshal { unmarshal: "xml",
schema }` for POST/PUT/PATCH, user steps, `Marshal { marshal: "xml" }`,
`SetHeader(Content-Type = produces.trim())`, `SetHeaderIfAbsent(status)`. Nothing
else in `lower_operation` changes; `json`/`raw`/`form` arms, their strings, and
their step order stay byte-identical. `request_schema` and `response.schema` are
allowed (no `raw`-style rejection).

**D3 — Format reuse, no new config.** `XmlDataFormat` is used with its defaults
(`max_depth = 100`). No new config surface: the format already fails closed on
over-depth documents, and the request stream is materialized by the existing
`StreamCacheService` wrap around `UnmarshalService`, exactly as for `json` and
`form`. `Body::Text`/`Body::Bytes`/`Body::Xml` unmarshal to `Body::Json`;
`Body::Json` marshals to `Body::Text`.

**D3b — Empty-body parity (additive, format-global).** `XmlDataFormat::marshal`
currently returns `TypeConversionFailed` for `Body::Empty`, while
`JsonDataFormat::marshal` passes `Body::Empty` through. A REST operation that
produces no body (`success_status: 204`, a DELETE handler that leaves the body
empty) would therefore 500 only in XML binding. Add `Body::Empty => Ok(Body::Empty)`
to `XmlDataFormat::marshal`, matching the `json` contract and its documented
rationale, with a unit test. This is a change to the format's global contract, not
an XML-binding-only rule: it also affects standalone `marshal: xml` EIP users, and
must be documented as format behavior in `docs/src/data-formats/xml.md`. The e2e
suite pins a 204 XML operation (empty reply body, declared Content-Type header,
default-status header). `Body::Bytes`, `Body::Stream`, and `Body::Xml` in marshal
keep failing closed (a response is expected to be JSON by then; a `Body::Xml` step
output is a documented gap, tracked as a deferral, not changed here).

**D3c — Request-side empty body stays fail-closed (recorded divergence).**
`XmlDataFormat::unmarshal` rejects `Body::Empty` with `TypeConversionFailed`,
whereas `JsonDataFormat` and `FormDataFormat` treat it as a no-op. That is kept
unchanged: an empty body is not well-formed XML, and the converter already reports
"empty XML: no root element found" for whitespace-only input. The divergence is
documented in `docs/src/data-formats/xml.md` and pinned by a test, and never
reaches user steps. It is recorded in the deferrals ledger as a recorded decision,
not a defect.

**D3d — DOCTYPE and entity references (observed behavior, pinned).**
`XmlDataFormat::unmarshal` calls `xml_to_json_with_depth_limit`, which swallows
`DOCTYPE` events (no DTD node, no external resolution) but unescapes general
references with quick-xml's predefined-entity table only. A `DOCTYPE` without
entity references therefore converts normally, while a reference to a
declared-but-unknown entity (`&x;`) fails closed with
`TypeConversionFailed("cannot unescape XML ref ...")`. That is the security
property to pin: no expansion, fail closed. The blessed spec scenario was
written assuming the reference would be silently dropped; it is corrected to the
observed fail-closed behavior before implementation. No `xml_convert` change.

**D4 — Schema ordering and payload shape.** Request path (runtime):
`unmarshal: xml` turns the XML body into `Body::Json`, then the existing
`JsonSchemaValidateService` wrap validates that JSON, then user steps run. A
mismatch is `CamelError::ValidationError` → 400. Response path: user steps produce
JSON, the declared `response.schema` is OpenAPI metadata only (no runtime response
validation exists for any binding), then `marshal: xml` renders XML.
Documentation must state that schemas describe the JSON projection produced by
`xml_convert`, not the wire XML. All projection rules are taken from
`crates/camel-api/src/xml_convert.rs::build_node_value` and
`insert_child`, not from memory:

- The root element becomes a single-key top-level object: `<user>…</user>` →
  `{"user": …}`.
- Attributes become `@name` keys with **string** values; `xmlns` declarations are
  dropped.
- Mixed content contributes a `#text` key; child elements with the same name
  accumulate into an array in document order.
- **Leaf text is always a JSON string** (`<n>42</n>` → `"42"`), never a number,
  boolean, or null. Authored schemas must type leaves as `type: string`; a
  `type: integer` leaf can never match. Whitespace-only/empty elements become
  `null`.

Marshal contract (from `xml_convert::json_to_xml`): the top-level value must be a
JSON object with exactly one non-`@`/non-`#text` key; that key is the root element
name. Zero or multiple element keys, a top-level array, and a top-level scalar all
fail with `TypeConversionFailed`. Inside the root, `@k` keys become attributes,
`#text` becomes text, other keys become child elements, arrays repeat the element,
and number/boolean leaves are stringified. Round-tripping is therefore asymmetric
for scalars (number → text → string); the docs state this and a test pins
multi-key rejection without changing behavior.

**D5 — OpenAPI parity.** `openapi.rs` already special-cases only `raw`
(`raw = binding == Raw`), so `Xml` flows through the authored-schema / weak-stub
path exactly as binding `json` does, and content is keyed by the declared
`consumes` and `produces`. No code change; a parity test pins it (authored schema
under the XML media key, weak-stub warning when absent). No `xml:` hint objects are
added. The docs warn that an OpenAPI schema keyed under `application/xml` still
describes the JSON projection, since that is what validation sees.

**D6 — Spec and docs.** `rest-dsl` delta: MODIFIED "REST operation binding mode
declaration" (closed set `json|raw|form|xml`) plus ADDED "XML binding mode media
acceptance" and "XML binding mode pipeline" requirements, with the OpenAPI parity
scenario inside the ADDED pipeline requirement. Docs: the REST binding table and
mode prose in `docs/src/yaml-dsl/top-level-blocks.md`, a new
`docs/src/data-formats/xml.md` linked from `docs/src/data-formats/index.md`
(schema ordering, JSON projection and leaf typing, marshal input contract, format
`Body::Empty` behavior in both directions, "XML is not multipart"), and the REST
DSL paragraph in `CONTEXT-MAP.md` (line ~187, currently enumerating json/raw/form).
The concrete check is a grep for `binding: raw`, `binding: form`, and
`json, raw` across `CONTEXT-MAP.md` and `docs/src`, with every hit updated.

## Affected surfaces

- `crates/camel-dsl/src/route_ast.rs` — enum variant.
- `crates/camel-dsl/src/media.rs` — `is_xml_media_type` + unit matrix.
- `crates/camel-dsl/src/rest.rs` — validation arm, format selector, lowering and
  error-message tests.
- `crates/camel-dsl/src/openapi.rs` — parity test only.
- `crates/camel-dsl/tests/rest_xml_e2e.rs` — executed pipeline (mirrors
  `rest_form_e2e.rs`).
- `crates/camel-processor/src/data_format/xml.rs` — `Body::Empty` marshal arm +
  tests.
- `docs/src/yaml-dsl/top-level-blocks.md`, `docs/src/data-formats/{index,xml}.md`.
- `openspec/changes/481-rest-xml-binding/specs/rest-dsl/spec.md`.

## Testing strategy

TDD per task. `is_xml_media_type` matrix (accept list, reject list above).
Lowering tests: full POST sequence with `xml`, GET without unmarshal, schema
allowed on request and response, actionable error text naming the operation and
field, unknown binding still fails at deserialize. Format test: empty-body marshal
passthrough plus the existing `xml.rs` suite. Executed end-to-end test: XML POST
reaches a step as `Body::Json`, response JSON leaves as XML text with the declared
Content-Type, malformed XML yields a typed error, schema violation yields a typed
error, a DTD-bearing document is not entity-expanded. Existing `json`/`raw`/`form`
suites run unchanged.

## Phases

Single delivery phase — one cohesive slice (approximately six tasks), no
cross-phase ordering hazard and no partially valid intermediate state.
