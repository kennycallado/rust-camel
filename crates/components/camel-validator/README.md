# camel-component-validator

> Validator component for rust-camel (XSD, JSON Schema, YAML)

## Overview

Validator component for rust-camel (XSD, JSON Schema, YAML).

Validator component for rust-camel supporting XSD, JSON Schema, and YAML schema validation.

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
| `type` | *(from extension)* | Schema type: `xml`, `json`, `yaml` |
| `maxPayloadBytes` | *(none)* | Reject bodies larger than this |
| `failOnNullBody` | `true` | Reject empty bodies when `true` |
| `headerName` | *(none)* | Validate a header value instead of the body |
| `failOnNullHeader` | `true` | Reject when the named header is missing |

## Startup behavior

Schema is compiled and cached on first validation call.

## Build requirement

XSD validation is delegated to `xml-bridge` (gRPC backend) and no longer depends on `libxml2` in this crate. The bridge process is cleaned up when the Camel context stops via the `Lifecycle` service.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-validator = "*"
```
