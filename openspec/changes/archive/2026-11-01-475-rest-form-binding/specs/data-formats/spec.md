# data-formats Delta

## ADDED Requirements

### Requirement: Form data format

The system SHALL provide a built-in `form` data format (`name() == "form"`)
that unmarshals an `application/x-www-form-urlencoded` body into a JSON
object and marshals a JSON object back into form text, under bounded
configuration (`max_fields`, default 1000, counting total non-empty pairs
with duplicates included; `max_bytes`, default 1 MiB, on input length).
The canon "Bounded format configuration" requirement's `Body::Empty`
rejection covers the archive formats only; the form format follows the
JSON format here (`Body::Empty` is a no-op).
Every parse or encode failure SHALL fail closed with a
`CamelError::TypeConversionFailed` whose message is prefixed `form` and
names the cap kind or the malformed escape with its byte offset. Unmarshal
SHALL accept `Body::Text` and `Body::Bytes`; `Body::Empty` SHALL be a no-op
returning `Body::Empty` (mirroring the JSON format); `Body::Stream` SHALL
fail closed and streams SHALL never be consumed. Pairs split on `&`; empty
segments (leading/trailing `&`, `&&`) are skipped and do not count toward
`max_fields`; each non-empty pair splits on the first `=`; a missing `=`
yields an empty value; an empty value is `""`; `+` decodes to space; `%XX`
escapes decode strictly (malformed escapes fail) and decoded bytes SHALL be
valid UTF-8. Duplicate keys SHALL become arrays with values in first-seen
(encounter) order — no lossy object overwrite. Object key iteration order
is unspecified; consumers compare by JSON value. Marshal: `Body::Empty` is
a no-op; only a top-level JSON object marshals — strings pass through,
number/bool/null stringify to their JSON lexical form, arrays repeat the
key per scalar element, nested objects or arrays SHALL be rejected, and
the empty object marshals to `""`. Marshal encoding SHALL follow the
WHATWG `application/x-www-form-urlencoded` serializer: ASCII alphanumerics
and `*-._` pass unescaped, space becomes `+`, every other byte becomes
uppercase `%XX` over its UTF-8 encoding.

#### Scenario: single pair unmarshals to a flat object

- **GIVEN** an exchange whose body is `Body::Text("name=camel")`
- **WHEN** the form data format unmarshals it
- **THEN** the body is a JSON object equal to `{"name": "camel"}`

#### Scenario: duplicate keys become arrays in encounter order

- **GIVEN** an exchange whose body is
  `Body::Text("tag=a&tag=b&tag=c")`
- **WHEN** the form data format unmarshals it
- **THEN** the body is a JSON object where `tag` equals the array
  `["a", "b", "c"]`

#### Scenario: plus and percent escapes decode

- **GIVEN** an exchange whose body is
  `Body::Text("a=one+two&b=one%20two&c=1%2B1")`
- **WHEN** the form data format unmarshals it
- **THEN** the JSON object equals `{"a": "one two", "b": "one two",
  "c": "1+1"}`

#### Scenario: missing equals yields an empty value

- **GIVEN** an exchange whose body is `Body::Text("flag&x=")`
- **WHEN** the form data format unmarshals it
- **THEN** the JSON object equals `{"flag": "", "x": ""}`

#### Scenario: empty key is allowed

- **GIVEN** an exchange whose body is `Body::Text("=v")`
- **WHEN** the form data format unmarshals it
- **THEN** the JSON object equals `{"": "v"}`

#### Scenario: empty input unmarshals to an empty object

- **GIVEN** an exchange whose body is `Body::Text("")`
- **WHEN** the form data format unmarshals it
- **THEN** the body is the JSON object `{}`

#### Scenario: empty segments are skipped

- **GIVEN** an exchange whose body is `Body::Text("a=1&&b=2")` or
  `Body::Text("&a=1&")`
- **WHEN** the form data format unmarshals it
- **THEN** the JSON object equals `{"a": "1", "b": "2"}` (respectively
  `{"a": "1"}`) and the empty segments do not count toward `max_fields`

#### Scenario: UTF-8 payload round trip

- **GIVEN** an exchange whose body is `Body::Text("nome=café")` encoded as
  UTF-8
- **WHEN** the form data format unmarshals it
- **THEN** the JSON object equals `{"nome": "café"}` and marshalling that
  object back yields `nome=caf%C3%A9`

#### Scenario: malformed percent escape fails closed

- **GIVEN** an exchange whose body is `Body::Text("a=100%2")` or
  `Body::Text("a=%zz")`
- **WHEN** the form data format unmarshals it
- **THEN** unmarshal fails with `CamelError::TypeConversionFailed` whose
  message starts with `form` and names the malformed escape

#### Scenario: invalid UTF-8 fails closed

- **GIVEN** an exchange whose body is `Body::Bytes(b"a=%FF")` or
  `Body::Bytes(b"a=%E9")` (lone continuation/leading byte)
- **WHEN** the form data format unmarshals it
- **THEN** unmarshal fails with `CamelError::TypeConversionFailed` whose
  message starts with `form`

#### Scenario: field cap fails closed and the exact cap passes

- **GIVEN** an exchange whose body contains more non-empty pairs than
  `max_fields`
- **WHEN** the form data format unmarshals it
- **THEN** unmarshal fails with `CamelError::TypeConversionFailed` whose
  message starts with `form` and names `max_fields`
- **GIVEN** an exchange whose body contains exactly `max_fields` non-empty
  pairs
- **WHEN** the form data format unmarshals it
- **THEN** unmarshal succeeds

#### Scenario: byte cap fails closed before parsing

- **GIVEN** an exchange whose body length exceeds `max_bytes`
- **WHEN** the form data format unmarshals it
- **THEN** unmarshal fails with `CamelError::TypeConversionFailed` whose
  message starts with `form` and names `max_bytes`, before any pair is
  parsed

#### Scenario: empty body passes through unchanged

- **GIVEN** an exchange whose body is `Body::Empty`
- **WHEN** the form data format unmarshals or marshals it
- **THEN** the body stays `Body::Empty` and no error is raised

#### Scenario: stream body fails closed

- **GIVEN** an exchange whose body is `Body::Stream`
- **WHEN** the form data format unmarshals or marshals it
- **THEN** the operation fails with `CamelError::TypeConversionFailed` and
  the stream is not consumed

#### Scenario: marshal flat object

- **GIVEN** an exchange whose body is the JSON object `{"a": "1", "b": "2"}`
- **WHEN** the form data format marshals it
- **THEN** the form text, re-parsed as pairs, equals the set
  `{a: "1", b: "2"}` joined by `&` (pair order unspecified)

#### Scenario: marshal array values repeat the key

- **GIVEN** an exchange whose body is the JSON object
  `{"tag": ["a", "b"]}`
- **WHEN** the form data format marshals it
- **THEN** the body is form text exactly `tag=a&tag=b`

#### Scenario: marshal scalar values stringify to their JSON lexical form

- **GIVEN** an exchange whose body is the JSON object
  `{"n": 1, "b": true, "z": null}`
- **WHEN** the form data format marshals it
- **THEN** the form text, re-parsed as a pair-set, equals
  `{n: "1", b: "true", z: "null"}`

#### Scenario: marshal encodes per the WHATWG serializer

- **GIVEN** an exchange whose body is the JSON object
  `{"q": "a b&c=d+é"}`
- **WHEN** the form data format marshals it
- **THEN** the body is form text exactly `q=a+b%26c%3Dd%2B%C3%A9`

#### Scenario: marshal rejects nested structures

- **GIVEN** an exchange whose body is the JSON object
  `{"a": {"b": "1"}}` or `{"a": [["1"]]}`
- **WHEN** the form data format marshals it
- **THEN** marshal fails with `CamelError::TypeConversionFailed` whose
  message starts with `form` and names the nested value

#### Scenario: marshal empty object yields empty text

- **GIVEN** an exchange whose body is the JSON object `{}`
- **WHEN** the form data format marshals it
- **THEN** the body is `Body::Text("")`

#### Scenario: marshal rejects non-object top-level values

- **GIVEN** an exchange whose body is a JSON array or scalar
- **WHEN** the form data format marshals it
- **THEN** marshal fails with `CamelError::TypeConversionFailed` whose
  message starts with `form`
