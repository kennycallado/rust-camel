## ADDED Requirements

### Requirement: XML data format marshal body acceptance

The built-in `xml` data format (`XmlDataFormat`, `name() == "xml"`) SHALL
marshal `Body::Json` to `Body::Text` containing XML, pass `Body::Text` and
`Body::Empty` through unchanged, and pass an already-serialized XML body
through unchanged: `Body::Xml` SHALL be accepted as-is, and `Body::Bytes`
SHALL be accepted only when the bytes are valid UTF-8, in which case the
bytes are passed through unchanged (the body shape stays `Body::Bytes`, with
no re-encoding and no conversion to `Body::Text`). `Body::Bytes` that is not
valid UTF-8 SHALL fail closed with a `CamelError::TypeConversionFailed` whose
message names `Body::Bytes`, states that the bytes are not valid UTF-8, and
reports the first invalid byte offset. `Body::Stream` SHALL fail closed
without being consumed. The step SHALL NOT parse or re-serialize a
`Body::Xml`/`Body::Bytes` payload, SHALL NOT validate XML well-formedness for
those bodies, and SHALL NOT sniff content types; the caller owns the
already-serialized wire form. The UTF-8-only rule matches the REST media
gate, which rejects non-UTF-8 XML charset declarations.

#### Scenario: Xml body passes through unchanged

- **GIVEN** an exchange whose body is
  `Body::Xml("<user><name>kenny</name></user>")`
- **WHEN** the XML data format marshals it
- **THEN** the resulting body is exactly the same `Body::Xml` value

#### Scenario: valid UTF-8 bytes pass through unchanged

- **GIVEN** an exchange whose body is `Body::Bytes` holding
  `<user><name>kenny</name></user>` encoded as UTF-8
- **WHEN** the XML data format marshals it
- **THEN** the resulting body is `Body::Bytes` with byte-identical content

#### Scenario: invalid UTF-8 bytes fail with a typed diagnostic

- **GIVEN** an exchange whose body is `Body::Bytes` containing the byte
  sequence `[0xFF, 0xFE]`
- **WHEN** the XML data format marshals it
- **THEN** marshalling fails with `CamelError::TypeConversionFailed` whose
  message contains `Body::Bytes`, `UTF-8`, and the first invalid byte offset
  (`0`)

#### Scenario: JSON body still marshals to XML text

- **GIVEN** an exchange whose body is `Body::Json({"user": {"name": "kenny"}})`
- **WHEN** the XML data format marshals it
- **THEN** the resulting body is `Body::Text("<user><name>kenny</name></user>")`

#### Scenario: stream body fails closed

- **GIVEN** an exchange whose body is `Body::Stream`
- **WHEN** the XML data format marshals it
- **THEN** marshalling fails with `CamelError::TypeConversionFailed` and the
  stream is never consumed
