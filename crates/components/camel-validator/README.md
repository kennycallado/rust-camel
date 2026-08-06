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

`validator:path/to/schema[?type=xml|json|yaml&failOnNullBody=true|false&headerName=X-Header&failOnNullHeader=true|false]`

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
