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

## Accepted but inactive options

`XjEndpointConfig::from_uri` accepts and stores `transformDirection` and
`resourceUri`. Endpoint creation does not pass either field to `XjEndpoint` or
`XjProducer`, so both options are silently ignored. Do not depend on them until
rc-1v0s either wires them into behavior or removes them from the public URI
surface.

## `#[non_exhaustive]` posture

ADR-0049 does not apply to this Component crate. Its default covers public
contract enums in `camel-api`, `camel-component-api`, and
`camel-language-api`. This crate exports `Direction` and `XjError`, but neither
is in that policy's contract-crate scope.
