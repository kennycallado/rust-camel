# camel-component-validator

> Schema validation component for rust-camel (XSD via gRPC bridge, JSON Schema, YAML, protobuf).

## Overview

Validates message bodies or header values against XSD, JSON Schema, YAML, and protobuf schemas.
XSD validation uses the `xml-bridge` gRPC backend. JSON, YAML, and protobuf validation run
in-process.

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

`validator:path/to/schema[?type=xml|json|yaml|protobuf&schemaType=<type>&message=<fully.qualified.Message>&failOnNullBody=true|false&headerName=X-Header&failOnNullHeader=true|false&schemaAliases=<percent-encoded JSON object>]`

If `type` and `schemaType` are omitted, schema type is inferred from the file extension (for
example `.xsd` selects xml, `.json` selects json, `.yaml`/`.yml` select yaml, and `.proto`
selects protobuf).

### URI options

| Option | Default | Description |
|--------|---------|-------------|
| `type` | *(from extension)* | Schema type: `xml`, `json`, `yaml`, or `protobuf`. `rng` and `schematron` are recognized but not yet supported. |
| `schemaType` | *(from extension)* | Canonical alias of `type`. Both keys are accepted and must agree when both are set. |
| `message` | *(none)* | Required for `protobuf`: fully-qualified message name, e.g. `test.v1.Person`. |
| `maxPayloadBytes` | *(none)* | Reject bodies larger than this. Accepts `maxPayloadBytes=` and `maxPayloadBytes:`. |
| `schemaCacheMaxEntries` | `256` | Maximum combined success + refusal cache entries before eviction. |
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

### Protobuf validation

```text
validator:schemas/person.proto?message=test.v1.Person
```

The protobuf validator accepts JSON-shaped bodies only. `Body::Json` is the primary input;
`Body::Text` and `Body::Bytes` are parsed as UTF-8 JSON first. Other body variants are rejected.

The body is never encoded or decoded as protobuf and never mutated. A passing body passes through
byte-identical, with no JSON normalization.

Unknown fields, wrong scalar, enum, or oneof shapes, malformed JSON, and non-UTF-8 bytes are
rejected with `CamelError::ValidationError`. The shared transport error mapping turns that into
HTTP 400 (`validation_error`), the same mapping that REST schema failures use. This is new-path
behavior and does not change how Xml, Json, or Yaml validator results are classified.

JSON `null` and missing proto3 fields follow proto3 default semantics. They are accepted, and the
fields fall back to their defaults.

The descriptor compiles once at endpoint creation through the hermetic proto compiler
(ADR-0084). Input may be `.proto` source or a precompiled descriptor set (`.binpb`, `.pb`, `.desc`,
`.protoset`). Include paths are empty. The existing path validation rejects `..` traversal.

A missing or empty `message` fails endpoint or route creation with a config error that names
`message`. A missing schema file or an uncompilable schema also fails endpoint creation.

## Startup behavior

JSON, YAML, and protobuf schemas compile when the Endpoint is created. Invalid schemas fail
Endpoint creation. XSD registration is deferred until the first validation call so bridge startup runs in
an async context. The compiled validator remains cached for the Endpoint's lifetime.

## XSD failure caching

The XSD bridge memoizes only deterministic schema refusals: responses whose kind is
`COMPILATION_FAILED`. A memoized refusal is keyed by schema-set identity and bridge generation, so
repeated registration of the same refused set returns the same typed error without another send or
compile. Transient transport errors and every other bridge response kind are never memoized; they
retry on the next attempt. The memo is cleared when the bridge restarts, so a refusal is re-attempted
once per bridge process. Refusals share the `schemaCacheMaxEntries` budget with the success cache
under a combined full-clear eviction, and refusal entries yield to successes. While the success cache
alone sits at capacity, new refusals are not cached and each Exchange retries as before. No new
configuration is required.

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
