# camel-component-validator

> Schema validation component for rust-camel (XSD via gRPC bridge, JSON Schema, YAML).

## Overview

Validates message bodies or header values against XSD, JSON Schema, and YAML schemas. XSD
validation uses the `xml-bridge` gRPC backend. JSON and YAML validation run in-process.

## Usage

```rust
// Validate XML body against XSD schema
route!{
    from("timer:tick?repeatCount=1")
    .to("validator:schemas/order.xsd")
}

// Validate JSON body
route!{
    from("timer:tick?repeatCount=1")
    .to("validator:schemas/order.json")
}

// Fail silently on empty body
route!{
    from("kafka:input")
    .to("validator:schemas/order.xsd?failOnNullBody=false")
}
```

## URI format

`validator:path/to/schema[?type=xml|json|yaml&failOnNullBody=true|false&headerName=X-Header&failOnNullHeader=true|false&schemaAliases=<percent-encoded JSON object>]`

If `type` is omitted, schema type is inferred from file extension.

### URI options

| Option | Default | Description |
|--------|---------|-------------|
| `type` | *(from extension)* | Schema type: `xml`, `json`, or `yaml`. `rng` and `schematron` are recognized but not yet supported. |
| `maxPayloadBytes` | *(none)* | Reject bodies larger than this. Accepts `maxPayloadBytes=` and `maxPayloadBytes:`. |
| `schemaCacheMaxEntries` | `256` | Maximum XSD schema entries before the bridge cache is evicted. |
| `failOnNullBody` | `true` | Reject empty bodies when `true` |
| `headerName` | *(none)* | Validate a header value instead of the body |
| `failOnNullHeader` | `true` | Reject when the named header is missing |
| `schemaAliases` | *(none)* | Absolute-URI to root-relative virtual path map, as one percent-encoded JSON object. See below. |

`schemaAliases` maps absolute schema URIs to root-relative virtual paths. The value is one JSON
object percent-encoded once. For example,
`schemaAliases=%7B%22http%3A%2F%2Fexample.com%2Fp.xsd%22%3A%22a%2Fparent.xsd%22%7D` decodes once to
`{"http://example.com/p.xsd":"a/parent.xsd"}`. A malformed value, a duplicated `schemaAliases`
query parameter, or more than 128 entries fail endpoint creation before the bridge starts. Alias
values are root-relative virtual paths. XSD discovery follows only the reachable
`xs:import`/`xs:include` edges inside the schema file's parent directory (the confined root). The
bounds fail closed during discovery: 256 resource entries, 8 MiB total bytes, 128 aliases, and
import depth 32.

Rust reads the confined schema files and transmits the schema set in memory.
The Java bridge resolves only from the transmitted in-memory set.
Its supplied-key resolver never fetches: no network and no filesystem access occur.

## Startup behavior

JSON and YAML schemas compile when the Endpoint is created. Invalid schemas fail Endpoint
creation. XSD registration is deferred until the first validation call so bridge startup runs in
an async context. The compiled validator remains cached for the Endpoint's lifetime.

## Build requirement

XSD validation is delegated to `xml-bridge` and does not depend on `libxml2` in this crate.
`CamelContext::stop()` does not clean up the bridge by itself. `camel run` handles cleanup through
its CLI-side `BridgeCleanup` lifecycle service. Library embedders must retain the value from
`ValidatorComponent::xsd_bridge_backend()` and call `shutdown().await` when it is `Some`.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-validator = "*"
```
