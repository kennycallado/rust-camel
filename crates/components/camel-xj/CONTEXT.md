# camel-xj

XML-to-JSON and JSON-to-XML conversion Component for the `xj:` URI scheme.

## Delegation model

The Component uses the `camel-xslt` client and the Java/Saxon `xml-bridge`
sidecar. Rust selects a bundled or configured stylesheet and sends the
stylesheet and document to the sidecar. Rust does not parse XML or execute
XSLT. XML parsing, DTD handling, entity resolution, and XSLT security controls
belong to the sidecar and must be audited in `bridges/xml/`.

For `json2xml`, Rust validates UTF-8 and forwards the JSON text as the
`jsonInput` XSLT parameter with a minimal XML document. It does not parse the
JSON document.

## Input bound

ADR-0040 governs body materialization. `maxPayloadBytes` sets the limit. When
the option is absent, `XjProducer::call` uses
`DEFAULT_MATERIALIZE_LIMIT` (10 MiB). It passes the effective limit to
`Body::into_bytes` before it sends input to the sidecar.

## Rejected options

`XjEndpointConfig::from_uri` rejects `transformDirection` and `resourceUri`
with an error. These options were previously accepted but silently ignored.
## `#[non_exhaustive]` posture

ADR-0049 does not apply to this Component crate. Its default covers public
contract enums in `camel-api`, `camel-component-api`, and
`camel-language-api`. This crate exports `Direction` and `XjError`, but neither
is in that policy's contract-crate scope.
